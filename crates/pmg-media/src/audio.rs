//! Audio decode: a second ffmpeg, PCM over a pipe, pulled by the video clock.
//!
//! # Why a separate process from the video
//!
//! One pipe cannot carry two raw streams without a container, and adding one
//! back just to strip it again buys nothing. So a clip with sound runs two
//! ffmpegs: one emitting RGBA frames, one emitting interleaved PCM.
//!
//! **That does not put sync at risk, and it is worth being precise about why.**
//! Both are seeked to the same media timestamp, and after that neither is
//! consulted about timing at all — the video clock decides that frame `n` gets
//! one picture and [`pmg_audio::AudioClock`] decides it gets exactly N samples.
//! Two deterministic counters driven from the same frame number cannot drift
//! apart. What two processes *can* do is start at different moments, which is an
//! offset rather than a drift, is bounded by the pre-roll, and is measurable.
//!
//! # Fixed at 48 kHz stereo, converted by ffmpeg
//!
//! `-ar 48000 -ac 2` makes ffmpeg do any resampling and downmixing, off our
//! thread and with a far better resampler than we would write. The frame path
//! then only ever slices a buffer, which is the whole point: **nothing on the
//! frame path may do arithmetic on samples.**

use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver};

use crate::MediaError;

/// Interleaved stereo, 32-bit signed, 48 kHz. The one shape everything downstream
/// expects, so that no conversion survives past this module.
pub const CHANNELS: usize = 2;
pub const SAMPLE_RATE: u32 = 48_000;

/// Bytes per interleaved stereo sample frame.
const STRIDE: usize = CHANNELS * 4;

/// How many sample-frames the reader hands over at a time.
///
/// A video frame at 59.94 wants 800 or 801, so 1024 is a little over one frame
/// and the queue below stays shallow in time rather than in count.
const CHUNK: usize = 1024;

/// Chunks the decoder may run ahead. About 85 ms at 48 kHz.
const QUEUE_DEPTH: usize = 4;

/// Whether a clip has usable audio, decided by ffprobe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioSpec {
    /// Channels in the SOURCE. We always output stereo; this is for reporting,
    /// so an operator can see that a 5.1 clip was downmixed rather than wonder.
    pub source_channels: u32,
    pub source_rate: u32,
}

/// A running ffmpeg producing PCM, and the samples it has produced.
pub struct AudioDecoder {
    child: Child,
    rx: Receiver<Vec<i32>>,
    /// Samples not yet handed to a frame. Never more than one chunk plus change.
    pending: Vec<i32>,
    stop: std::sync::Arc<AtomicBool>,
    ended: std::sync::Arc<AtomicBool>,
    produced: std::sync::Arc<AtomicU64>,
    /// Sample-frames handed out. The audio counterpart of the video frame index.
    consumed: u64,
    /// Times a frame was served short because nothing had arrived.
    starved: u64,
}

impl AudioDecoder {
    /// Spawn ffmpeg positioned at the same media time as the video decoder.
    ///
    /// `start_frame` is a **video** frame index and is converted here, so the
    /// caller cannot accidentally seek the two streams to different places.
    pub fn open(
        path: impl AsRef<Path>,
        video_fps: (u32, u32),
        start_frame: u64,
        looping: bool,
    ) -> Result<Self, MediaError> {
        let mut cmd = Command::new("ffmpeg");
        cmd.args(["-hide_banner", "-loglevel", "error", "-nostdin"]);
        if looping {
            cmd.args(["-stream_loop", "-1"]);
        }
        if start_frame > 0 {
            let t = start_frame as f64 * video_fps.1 as f64 / video_fps.0.max(1) as f64;
            cmd.args(["-ss", &format!("{t:.6}")]);
        }
        cmd.arg("-i").arg(path.as_ref());
        cmd.args([
            // No video down this pipe; the other process owns that.
            "-vn",
            // ffmpeg resamples and downmixes. Better than anything we would
            // write, and it happens on its thread rather than the frame path.
            "-ar", "48000", "-ac", "2", "-f", "s32le", "-",
        ]);
        cmd.stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null());

        let mut child = cmd
            .spawn()
            .map_err(|e| MediaError::NoFfmpeg(format!("ffmpeg (audio): {e}")))?;
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| MediaError::NoFfmpeg("ffmpeg gave no stdout".into()))?;

        let (tx, rx) = sync_channel::<Vec<i32>>(QUEUE_DEPTH);
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let ended = std::sync::Arc::new(AtomicBool::new(false));
        let produced = std::sync::Arc::new(AtomicU64::new(0));
        let (t_stop, t_ended, t_produced) = (stop.clone(), ended.clone(), produced.clone());

        std::thread::Builder::new()
            .name("pmg-decode-audio".into())
            .spawn(move || {
                let mut buf = vec![0u8; CHUNK * STRIDE];
                loop {
                    if t_stop.load(Ordering::Relaxed) {
                        break;
                    }
                    // A partial read is the end of the stream. Unlike video,
                    // where a partial frame is useless, a partial chunk is
                    // perfectly good audio and must be passed on — dropping it
                    // would clip the last 20 ms off every clip.
                    let mut got = 0usize;
                    while got < buf.len() {
                        match stdout.read(&mut buf[got..]) {
                            Ok(0) | Err(_) => break,
                            Ok(n) => got += n,
                        }
                    }
                    let frames = got / STRIDE;
                    if frames > 0 {
                        let samples: Vec<i32> = buf[..frames * STRIDE]
                            .chunks_exact(4)
                            .map(|b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                            .collect();
                        t_produced.fetch_add(frames as u64, Ordering::Relaxed);
                        if tx.send(samples).is_err() {
                            return;
                        }
                    }
                    if got < buf.len() {
                        break;
                    }
                }
                t_ended.store(true, Ordering::Relaxed);
            })
            .map_err(|e| MediaError::NoFfmpeg(e.to_string()))?;

        Ok(Self {
            child,
            rx,
            pending: Vec::with_capacity(CHUNK * CHANNELS * 2),
            stop,
            ended,
            produced,
            consumed: 0,
            starved: 0,
        })
    }

    /// Sample-frames handed out so far. Compare against
    /// [`pmg_audio::AudioClock::samples_before`] to see whether audio is where
    /// the video clock says it should be.
    pub fn consumed(&self) -> u64 {
        self.consumed
    }

    pub fn produced(&self) -> u64 {
        self.produced.load(Ordering::Relaxed)
    }

    /// Frames served with less audio than they asked for.
    pub fn starved(&self) -> u64 {
        self.starved
    }

    pub fn ended(&self) -> bool {
        self.ended.load(Ordering::Relaxed)
    }

    /// Block until some audio exists. **Call at arm**, alongside the video
    /// pre-roll and for the same reason.
    pub fn preroll(&mut self, timeout: std::time::Duration) -> bool {
        if !self.pending.is_empty() {
            return true;
        }
        match self.rx.recv_timeout(timeout) {
            Ok(chunk) => {
                self.pending.extend_from_slice(&chunk);
                true
            }
            Err(_) => false,
        }
    }

    /// Take exactly `sample_frames` interleaved stereo frames for one video frame.
    ///
    /// **Never blocks.** Short is padded with silence and counted: a frame of
    /// quiet is a blemish, and a render loop waiting on a decoder is a dropped
    /// frame on air. The returned buffer is always exactly the requested length,
    /// so a caller cannot accidentally send a short buffer to a card that will
    /// interpret it as a rate change.
    pub fn take(&mut self, sample_frames: usize) -> Vec<i32> {
        let want = sample_frames * CHANNELS;
        while self.pending.len() < want {
            match self.rx.try_recv() {
                Ok(chunk) => self.pending.extend_from_slice(&chunk),
                Err(_) => break,
            }
        }
        let have = self.pending.len().min(want);
        let mut out: Vec<i32> = self.pending.drain(..have).collect();
        if have < want {
            self.starved += 1;
            out.resize(want, 0);
        }
        self.consumed += sample_frames as u64;
        out
    }
}

impl Drop for AudioDecoder {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Does this file have an audio stream, and what is it?
pub fn probe_audio(path: impl AsRef<Path>) -> Result<Option<AudioSpec>, MediaError> {
    let out = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-print_format",
            "json",
            "-select_streams",
            "a:0",
            "-show_streams",
        ])
        .arg(path.as_ref())
        .output()
        .map_err(|e| MediaError::NoFfmpeg(format!("ffprobe: {e}")))?;
    if !out.status.success() {
        return Err(MediaError::Probe(
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }
    let json: serde_json::Value =
        serde_json::from_slice(&out.stdout).map_err(|e| MediaError::Probe(e.to_string()))?;
    let Some(s) = json["streams"].as_array().and_then(|a| a.first()) else {
        // No audio is a normal property of a graphic, not a failure.
        return Ok(None);
    };
    Ok(Some(AudioSpec {
        source_channels: s["channels"].as_u64().unwrap_or(0) as u32,
        source_rate: s["sample_rate"]
            .as_str()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn have_ffmpeg() -> bool {
        Command::new("ffmpeg")
            .arg("-version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    }

    /// A clip with a tone, at a rate and channel count we do NOT want, so the
    /// conversion is actually exercised rather than assumed.
    fn fixture(dir: &Path) -> Option<std::path::PathBuf> {
        let out = dir.join("tone.mov");
        let ok = Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "color=c=black:s=64x48:r=30:d=2",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=1000:sample_rate=44100:duration=2",
                "-c:v",
                "prores_ks",
                "-profile:v",
                "3",
                "-c:a",
                "pcm_s16le",
            ])
            .arg(&out)
            .status()
            .ok()?;
        ok.success().then_some(out)
    }

    #[test]
    fn a_clip_with_no_audio_says_so_rather_than_failing() {
        if !have_ffmpeg() {
            eprintln!("skipped: no ffmpeg on PATH");
            return;
        }
        let dir = std::env::temp_dir().join(format!("pmg-audio-silent-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("silent.mov");
        let ok = Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "color=c=black:s=32x32:r=30:d=1",
                "-c:v",
                "prores_ks",
            ])
            .arg(&out)
            .status()
            .is_ok_and(|s| s.success());
        if ok {
            assert_eq!(probe_audio(&out).unwrap(), None, "silence is not an error");
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn audio_is_converted_to_48k_stereo_and_served_in_exact_counts() {
        if !have_ffmpeg() {
            eprintln!("skipped: no ffmpeg on PATH");
            return;
        }
        let dir = std::env::temp_dir().join(format!("pmg-audio-tone-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let Some(path) = fixture(&dir) else {
            eprintln!("skipped: could not build a fixture clip");
            return;
        };

        let spec = probe_audio(&path).unwrap().expect("the fixture has a tone");
        assert_eq!(
            spec.source_rate, 44_100,
            "the source is deliberately not 48k"
        );

        let mut dec = AudioDecoder::open(&path, (60000, 1001), 0, false).expect("open");
        assert!(
            dec.preroll(std::time::Duration::from_secs(20)),
            "no audio arrived"
        );

        // Serve one cadence period at 59.94 and check the counts are the
        // clock's, not the decoder's.
        let clock = pmg_audio::AudioClock::new((60000, 1001), pmg_audio::SAMPLE_RATE).unwrap();
        let mut total = 0usize;
        for n in 0..5u64 {
            let want = clock.samples_for(n) as usize;
            let got = dec.take(want);
            assert_eq!(
                got.len(),
                want * CHANNELS,
                "take must return exactly what was asked for"
            );
            total += want;
        }
        assert_eq!(total, 4004, "five frames at 59.94 carry 4004 samples");
        assert_eq!(dec.consumed(), 4004);

        // Give the decoder a moment, then confirm there is real signal — a
        // 1 kHz tone converted to 48 kHz must not be silence.
        std::thread::sleep(std::time::Duration::from_millis(200));
        let more = dec.take(800);
        assert!(
            more.iter().any(|&s| s.abs() > 1_000_000),
            "a 1 kHz tone should not decode to near-silence"
        );

        std::fs::remove_dir_all(&dir).ok();
    }
}
