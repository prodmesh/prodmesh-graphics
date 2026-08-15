//! Video decode: ffmpeg as a **child process**, raw premultiplied RGBA over a pipe.
//!
//! # Why a process and not a library
//!
//! `docs/RISKS.md` R5 recommends `rsmpeg`/`ffmpeg-next` — FFI into libavcodec.
//! Two things measured on 2026-08-15 argue for a pipe instead, at least for now.
//!
//! **Licensing, which is the one that actually decides it.** The ffmpeg on this
//! machine is built `--enable-gpl --enable-version3` (Homebrew 7.1.1, the
//! default). Linking that in-process makes the whole binary GPL. R5 says "LGPL
//! components only, dynamically linked, no GPL" and ADR 0003 keeps that as a
//! productization seam — and a seam you close by accident, at build time, on the
//! developer's own machine, is not a seam. Invoking a separate program over a
//! pipe is the ordinary aggregation boundary and closes nothing.
//!
//! **Performance, which turns out not to argue against it.** 1080p, 1200 frames,
//! Apple M-series, otherwise idle:
//!
//! | Source | Through the pipe | vs 59.94 |
//! |---|---|---|
//! | ProRes 422 HQ, no alpha | 736 fps | 12.3x |
//! | ProRes 4444, alpha, premultiplied | 168 fps | **2.8x** |
//! | seek + one frame, cold spawn | 80 ms | — |
//!
//! 2.8x on the case that matters — keyed video — is enough and is not enormous.
//! The 4444 clip cost ~9.4 cores to do it, which is the number to watch: it is
//! decode competing with a render loop on the same box, not a pipe bottleneck.
//! **Revisit FFI when a node must play several 4K clips at once, or when the
//! frame needs to reach the GPU without a trip through system RAM.** Neither is
//! true yet, and neither is made harder by having started here.
//!
//! # The rules this module obeys
//!
//! - **The output is the clock.** The decoder is *pulled* by the render loop and
//!   never pushes. Pipe backpressure is what throttles ffmpeg: a bounded channel
//!   fills, the reader thread stops reading, ffmpeg blocks on write.
//! - **Underrun holds the last frame.** It is logged and counted, never allowed
//!   to stall an output. A repeated frame is a blemish; a late frame is a drop.
//! - **Premultiplied at the boundary**, like every other pixel that enters the
//!   system (ADR 0012) — done by ffmpeg's own filter rather than a byte loop of
//!   ours, because it is off our thread and already vectorised.
//! - **Seek happens at arm, never per frame.** 80 ms is fine off air and absurd
//!   on it. Opening the decoder early is the pre-roll.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver};
use std::sync::Arc;

use crate::MediaError;

/// What a clip is, as far as playout is concerned.
#[derive(Debug, Clone, PartialEq)]
pub struct VideoSpec {
    pub codec: String,
    pub width: u32,
    pub height: u32,
    /// Exact rational frame rate. **Never a float** — 59.94 is 60000/1001, and
    /// rounding it is how a two-hour clip ends up a frame out.
    pub fps_num: u32,
    pub fps_den: u32,
    pub duration_s: f64,
    pub has_alpha: bool,
    /// Total frames, when the container says so. `0` means unknown, which is
    /// legal and only costs us the ability to report progress.
    pub frames: u64,
}

impl VideoSpec {
    pub fn fps(&self) -> f64 {
        if self.fps_den == 0 {
            0.0
        } else {
            self.fps_num as f64 / self.fps_den as f64
        }
    }

    /// Bytes in one RGBA frame.
    pub fn frame_bytes(&self) -> usize {
        self.width as usize * self.height as usize * 4
    }
}

/// One decoded frame, premultiplied RGBA8.
#[derive(Clone)]
pub struct DecodedFrame {
    /// Frames emitted since the decoder was opened, starting at 0.
    ///
    /// Monotonic even when looping — `-stream_loop` makes the wrap invisible
    /// here, which is what lets the clock stay a plain multiply.
    pub index: u64,
    pub rgba: Arc<Vec<u8>>,
}

impl std::fmt::Debug for DecodedFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DecodedFrame")
            .field("index", &self.index)
            .field("bytes", &self.rgba.len())
            .finish()
    }
}

/// How deep the decoder may run ahead.
///
/// Three frames is 25 MB at 1080p and about 50 ms of lead at 59.94 — enough to
/// ride out a slow decode without letting the decoder build latency it can never
/// give back. A deeper queue does not make playback smoother, it makes a seek
/// stale.
const QUEUE_DEPTH: usize = 3;

fn queue_depth() -> usize {
    std::env::var("PMG_DECODE_QUEUE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(QUEUE_DEPTH)
        .clamp(1, 64)
}

/// Read the container without decoding it.
pub fn probe(path: impl AsRef<Path>) -> Result<VideoSpec, MediaError> {
    let path = path.as_ref();
    let out = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_streams",
            "-show_format",
        ])
        .arg(path)
        .output()
        .map_err(|e| MediaError::NoFfmpeg(format!("ffprobe: {e}")))?;
    if !out.status.success() {
        return Err(MediaError::Probe(
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }
    let json: serde_json::Value =
        serde_json::from_slice(&out.stdout).map_err(|e| MediaError::Probe(e.to_string()))?;

    let stream = json["streams"]
        .as_array()
        .and_then(|s| s.iter().find(|s| s["codec_type"] == "video"))
        .ok_or(MediaError::NoVideoStream)?;

    let (fps_num, fps_den) = rational(stream["r_frame_rate"].as_str().unwrap_or("0/1"));
    if fps_num == 0 || fps_den == 0 {
        return Err(MediaError::Probe("no usable frame rate".into()));
    }

    // Variable frame rate is refused rather than accommodated (R5). A VFR source
    // has no answer to "which frame is at 4.000 s", so every cue against it is
    // approximately right, which is the failure mode this whole path exists to
    // avoid. `avg` differing from `r` by more than a hair is the signal.
    let (avg_num, avg_den) = rational(stream["avg_frame_rate"].as_str().unwrap_or("0/1"));
    if avg_num != 0 && avg_den != 0 {
        let r = fps_num as f64 / fps_den as f64;
        let a = avg_num as f64 / avg_den as f64;
        if (r - a).abs() / r > 0.01 {
            return Err(MediaError::VariableFrameRate { nominal: r, avg: a });
        }
    }

    let pix_fmt = stream["pix_fmt"].as_str().unwrap_or("");
    let duration_s = stream["duration"]
        .as_str()
        .or_else(|| json["format"]["duration"].as_str())
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(0.0);

    let frames = stream["nb_frames"]
        .as_str()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or_else(|| (duration_s * fps_num as f64 / fps_den as f64).round() as u64);

    Ok(VideoSpec {
        codec: stream["codec_name"].as_str().unwrap_or("?").to_string(),
        width: stream["width"].as_u64().unwrap_or(0) as u32,
        height: stream["height"].as_u64().unwrap_or(0) as u32,
        fps_num,
        fps_den,
        duration_s,
        has_alpha: has_alpha(pix_fmt),
        frames,
    })
}

/// `"60000/1001"` → `(60000, 1001)`.
fn rational(s: &str) -> (u32, u32) {
    let mut it = s.split('/');
    let n = it.next().and_then(|v| v.parse().ok()).unwrap_or(0);
    let d = it.next().and_then(|v| v.parse().ok()).unwrap_or(1);
    (n, d)
}

/// Does this pixel format carry an alpha plane?
///
/// The pixel format is the only reliable signal. A container claiming alpha it
/// does not have is common, and believing it costs a premultiply pass over every
/// frame for nothing.
fn has_alpha(pix_fmt: &str) -> bool {
    pix_fmt.starts_with("yuva")
        || pix_fmt.starts_with("rgba")
        || pix_fmt.starts_with("bgra")
        || pix_fmt.starts_with("argb")
        || pix_fmt.starts_with("abgr")
        || pix_fmt.starts_with("ya")
        || pix_fmt.contains("gbrap")
}

/// A running ffmpeg, its reader thread, and the frames it has produced.
pub struct Decoder {
    child: Child,
    rx: Receiver<DecodedFrame>,
    spec: VideoSpec,
    stop: Arc<AtomicBool>,
    /// Frames the reader produced. Read for diagnostics only.
    produced: Arc<AtomicU64>,
    ended: Arc<AtomicBool>,
    path: PathBuf,
}

impl Decoder {
    /// Spawn ffmpeg positioned at `start_frame` and start filling the queue.
    ///
    /// Called at **arm**. The 80 ms this costs is a pre-roll, paid off air, and
    /// by the time a take arrives the first frames are already in the channel.
    pub fn open(
        path: impl AsRef<Path>,
        spec: &VideoSpec,
        start_frame: u64,
        looping: bool,
    ) -> Result<Self, MediaError> {
        let path = path.as_ref().to_path_buf();
        let frame_bytes = spec.frame_bytes();
        if frame_bytes == 0 {
            return Err(MediaError::Probe("clip has no dimensions".into()));
        }

        let mut cmd = Command::new("ffmpeg");
        cmd.args(["-hide_banner", "-loglevel", "error", "-nostdin"]);

        // Looping is ffmpeg's job, not ours. Re-spawning at EOF would put an
        // 80 ms hole in every loop point; `-stream_loop` has none, and it keeps
        // the frame index monotonic so the clock never has to know.
        if looping {
            cmd.args(["-stream_loop", "-1"]);
        }

        // Input seek: for all-intra media this lands on the exact frame, because
        // every frame is a keyframe. That is the property ingest exists to
        // guarantee (`server/ingest.js`), spent here.
        if start_frame > 0 {
            let t = start_frame as f64 * spec.fps_den as f64 / spec.fps_num as f64;
            cmd.args(["-ss", &format!("{t:.6}")]);
        }

        cmd.arg("-i").arg(&path);
        // Audio is a separate path (M3e). Pulling it down this pipe would
        // interleave two rates in one stream and make both worse.
        cmd.arg("-an");

        // Premultiply only when there is alpha to premultiply. On an opaque clip
        // the filter is a no-op that still costs a full pass over every pixel —
        // measured at 2.8x the decode on its own.
        let filter = if spec.has_alpha {
            "format=rgba,premultiply=inplace=1"
        } else {
            "format=rgba"
        };
        cmd.args(["-vf", filter, "-f", "rawvideo", "-pix_fmt", "rgba", "-"]);

        cmd.stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null());

        let mut child = cmd
            .spawn()
            .map_err(|e| MediaError::NoFfmpeg(format!("ffmpeg: {e}")))?;
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| MediaError::NoFfmpeg("ffmpeg gave no stdout".into()))?;

        let (tx, rx) = sync_channel::<DecodedFrame>(queue_depth());
        let stop = Arc::new(AtomicBool::new(false));
        let ended = Arc::new(AtomicBool::new(false));
        let produced = Arc::new(AtomicU64::new(0));

        let (t_stop, t_ended, t_produced) = (stop.clone(), ended.clone(), produced.clone());
        std::thread::Builder::new()
            .name("pmg-decode".into())
            .spawn(move || {
                let mut index = 0u64;
                let mut buf = vec![0u8; frame_bytes];
                loop {
                    if t_stop.load(Ordering::Relaxed) {
                        break;
                    }
                    // A short read is end of stream, not an error: ffmpeg closes
                    // the pipe on the last frame boundary.
                    if stdout.read_exact(&mut buf).is_err() {
                        break;
                    }
                    let frame = DecodedFrame {
                        index,
                        rgba: Arc::new(buf.clone()),
                    };
                    index += 1;
                    t_produced.store(index, Ordering::Relaxed);

                    // Blocking send is the backpressure: when the consumer is
                    // behind, this thread parks, the pipe fills, and ffmpeg
                    // blocks on write. That is the whole flow-control story and
                    // it needs no rate limiter of our own.
                    //
                    // It blocks rather than polling because a thread parked on a
                    // channel is a thread not burning a core to learn nothing.
                    // (The first version polled with a 2 ms sleep; swapping it
                    // for this changed the measured held-frame count by zero.
                    // The holds were the missing pre-roll — see
                    // `VideoPlayer::preroll`.) An error here is the player
                    // having been dropped, which is the only exit needed.
                    if tx.send(frame).is_err() {
                        return;
                    }
                }
                t_ended.store(true, Ordering::Relaxed);
            })
            .map_err(|e| MediaError::NoFfmpeg(e.to_string()))?;

        Ok(Self {
            child,
            rx,
            spec: spec.clone(),
            stop,
            produced,
            ended,
            path,
        })
    }

    pub fn spec(&self) -> &VideoSpec {
        &self.spec
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn produced(&self) -> u64 {
        self.produced.load(Ordering::Relaxed)
    }

    /// Has the source run out? Always false while looping.
    pub fn ended(&self) -> bool {
        self.ended.load(Ordering::Relaxed)
    }

    fn try_next(&self) -> Option<DecodedFrame> {
        self.rx.try_recv().ok()
    }

    /// Block until the first frame is out, or give up.
    ///
    /// **This is the only place in the playback path allowed to block**, and it
    /// is called at arm, off air. See [`VideoPlayer::preroll`] for why skipping
    /// it is not a small optimisation but a visible fault.
    fn wait_first(&self, timeout: std::time::Duration) -> Option<DecodedFrame> {
        self.rx.recv_timeout(timeout).ok()
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // Kill rather than wait: ffmpeg is blocked writing into a pipe nobody
        // will read again, so waiting for it politely means waiting forever.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Maps output frames to media frames, in exact integer arithmetic.
///
/// The output is the clock (R5), so this only ever answers "given that the
/// output is on its *n*th frame since this clip started, which media frame
/// should be showing". 30p media on a 59.94p output repeats frames; 60p media on
/// a 25p output drops them. Both are deterministic and neither needs a float.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MediaClock {
    out_num: u32,
    out_den: u32,
    med_num: u32,
    med_den: u32,
}

impl MediaClock {
    pub fn new(out_fps: (u32, u32), media_fps: (u32, u32)) -> Self {
        Self {
            out_num: out_fps.0.max(1),
            out_den: out_fps.1.max(1),
            med_num: media_fps.0.max(1),
            med_den: media_fps.1.max(1),
        }
    }

    /// Media frame index for the `n`th output frame since the clip started.
    ///
    /// `floor(n * out_den * med_num / (out_num * med_den))`, in `u128` so a
    /// two-hour clip at 60000/1001 cannot overflow the intermediate. Using
    /// seconds as `f64` here is the classic way to be one frame out at the end
    /// of a long clip, and it costs nothing to not do it.
    pub fn media_frame(&self, out_frame: u64) -> u64 {
        let n = out_frame as u128 * self.out_den as u128 * self.med_num as u128;
        let d = self.out_num as u128 * self.med_den as u128;
        (n / d) as u64
    }
}

/// A decoder plus the frame currently showing.
///
/// Owns the "never stall" rule: [`VideoPlayer::frame_for`] returns something on
/// every call, from the moment the first frame lands.
pub struct VideoPlayer {
    dec: Decoder,
    clock: MediaClock,
    current: Option<DecodedFrame>,
    /// Output frames that reused the previous picture because nothing had
    /// arrived. Counted and reported — a preview that stutters and a node that
    /// says nothing is a bug you diagnose twice.
    held: u64,
    /// Decoded frames thrown away because the output had already moved past
    /// them. Normal when media runs faster than the output.
    dropped: u64,
    looping: bool,
    finished: bool,
}

impl VideoPlayer {
    pub fn new(dec: Decoder, out_fps: (u32, u32), looping: bool) -> Self {
        let clock = MediaClock::new(out_fps, (dec.spec.fps_num, dec.spec.fps_den));
        Self {
            dec,
            clock,
            current: None,
            held: 0,
            dropped: 0,
            looping,
            finished: false,
        }
    }

    pub fn spec(&self) -> &VideoSpec {
        self.dec.spec()
    }

    /// Wait for the first frame before the clock starts. **Call this at arm.**
    ///
    /// Measured on 2026-08-15: skipping it costs about 120 ms — ffmpeg's spawn
    /// and first decode — and the damage is not the missing 120 ms. The clock
    /// starts at output frame 0 while the decoder's frame 0 arrives at output
    /// frame 7, so the player is permanently seven frames behind and spends the
    /// next stretch sprinting to catch up, holding and discarding pictures as it
    /// goes. On a 10 s clip that showed up as 6 held and 12 dropped frames that
    /// looked exactly like an underrun and were not: a deeper queue changed
    /// nothing at all (3, 8 and 24 gave identical counts), which is what ruled
    /// starvation out.
    ///
    /// Arming before taking is what makes this free, and it is the same split
    /// that already pays for layout and shaping. Returns whether a frame landed.
    pub fn preroll(&mut self, timeout: std::time::Duration) -> bool {
        if self.current.is_some() {
            return true;
        }
        match self.dec.wait_first(timeout) {
            Some(f) => {
                self.current = Some(f);
                true
            }
            None => false,
        }
    }

    pub fn held(&self) -> u64 {
        self.held
    }

    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// True once a non-looping clip has played past its last frame.
    pub fn finished(&self) -> bool {
        self.finished
    }

    /// The picture for the `n`th output frame since this clip started.
    ///
    /// **Never blocks and never returns `None` once playing has begun.** If the
    /// decoder has not kept up, the previous frame is held and counted. The one
    /// thing this must not do is make the output wait.
    pub fn frame_for(&mut self, out_frame: u64) -> Option<&DecodedFrame> {
        let want = self.clock.media_frame(out_frame);

        // Already showing it — the common case when media runs slower than the
        // output, which is most of the time.
        if self.current.as_ref().is_some_and(|f| f.index == want) {
            return self.current.as_ref();
        }

        // Pull forward, discarding anything the output has already passed.
        loop {
            match self.dec.try_next() {
                Some(f) => {
                    let reached = f.index >= want;
                    if self.current.is_some() && !reached {
                        self.dropped += 1;
                    }
                    self.current = Some(f);
                    if reached {
                        break;
                    }
                }
                None => {
                    if self.current.is_some() {
                        // Underrun, or the clip has ended. Hold, count, carry on.
                        if self.dec.ended() && !self.looping {
                            self.finished = true;
                        } else {
                            self.held += 1;
                        }
                    }
                    break;
                }
            }
        }

        if !self.looping
            && self.dec.spec.frames > 0
            && want >= self.dec.spec.frames
            && self.dec.ended()
        {
            self.finished = true;
        }

        self.current.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rational_parses_broadcast_rates() {
        assert_eq!(rational("60000/1001"), (60000, 1001));
        assert_eq!(rational("25/1"), (25, 1));
        assert_eq!(rational("garbage"), (0, 1));
    }

    #[test]
    fn alpha_is_read_from_the_pixel_format() {
        assert!(has_alpha("yuva444p10le"));
        assert!(has_alpha("rgba"));
        assert!(!has_alpha("yuv422p10le"));
        // The trap: 4444 without alpha is a real and common pixel format, and
        // the profile name says nothing about it.
        assert!(!has_alpha("yuv444p10le"));
    }

    #[test]
    fn same_rate_is_one_to_one() {
        let c = MediaClock::new((60000, 1001), (60000, 1001));
        for n in [0u64, 1, 1000, 431_000] {
            assert_eq!(c.media_frame(n), n);
        }
    }

    #[test]
    fn thirty_on_sixty_repeats_every_frame_exactly_twice() {
        let c = MediaClock::new((60, 1), (30, 1));
        assert_eq!(c.media_frame(0), 0);
        assert_eq!(c.media_frame(1), 0);
        assert_eq!(c.media_frame(2), 1);
        assert_eq!(c.media_frame(3), 1);
    }

    #[test]
    fn sixty_on_twentyfive_drops_deterministically() {
        let c = MediaClock::new((25, 1), (60, 1));
        assert_eq!(c.media_frame(0), 0);
        assert_eq!(c.media_frame(1), 2);
        assert_eq!(c.media_frame(2), 4);
    }

    /// The reason the arithmetic is integer. Two hours of 59.94 into 29.97 media
    /// must land on exactly half the frame count — a float seconds conversion
    /// drifts here, and being one frame out at the end of a service is the kind
    /// of bug nobody finds until it matters.
    #[test]
    fn two_hours_of_pulldown_does_not_drift() {
        let c = MediaClock::new((60000, 1001), (30000, 1001));
        let two_hours = 431_568u64; // 2 h at 59.94
        assert_eq!(c.media_frame(two_hours), two_hours / 2);
        assert_eq!(c.media_frame(two_hours - 1), (two_hours - 1) / 2);
    }

    /// Is there an ffmpeg to test against?
    ///
    /// The decode tests below build a real clip and play it. They are **skipped**
    /// rather than failed when ffmpeg is absent, because a CI runner without it
    /// is a missing tool and not a broken decoder — and a test that fails for the
    /// wrong reason gets muted, which costs the coverage permanently.
    fn have_ffmpeg() -> bool {
        Command::new("ffmpeg")
            .arg("-version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    }

    /// A tiny clip with a real alpha channel whose frames are **provably
    /// different**: red is a function of the frame number.
    ///
    /// The first version used `testsrc2` and asserted consecutive frames
    /// differed. At 64x48 they did not, and the test failed while the decoder
    /// was working perfectly — a fixture that cannot distinguish "advanced" from
    /// "stuck" is worse than no fixture, because it fails for a reason that has
    /// nothing to do with the thing under test.
    fn fixture(dir: &Path, frames: u32) -> Option<PathBuf> {
        let out = dir.join("fixture.mov");
        let ok = Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                &format!("color=c=black:s=64x48:r=30:d={}", frames as f64 / 30.0),
                "-vf",
                // N is the frame index. Red steps 8 per frame, alpha is a flat
                // half — which makes both "did it advance" and "was it
                // premultiplied" answerable from one pixel.
                "format=rgba,geq=r='mod(N*8,256)':g='64':b='192':a='128'",
                "-c:v",
                "prores_ks",
                "-profile:v",
                "4",
                "-pix_fmt",
                "yuva444p10le",
            ])
            .arg(&out)
            .status()
            .ok()?;
        ok.success().then_some(out)
    }

    #[test]
    fn a_real_clip_probes_decodes_and_advances() {
        if !have_ffmpeg() {
            eprintln!("skipped: no ffmpeg on PATH");
            return;
        }
        let dir = std::env::temp_dir().join(format!("pmg-video-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let Some(path) = fixture(&dir, 30) else {
            eprintln!("skipped: could not build a fixture clip");
            return;
        };

        let spec = probe(&path).expect("probe");
        assert_eq!((spec.width, spec.height), (64, 48));
        assert_eq!((spec.fps_num, spec.fps_den), (30, 1));
        assert!(spec.has_alpha, "the fixture is yuva444p10le");

        let dec = Decoder::open(&path, &spec, 0, false).expect("open");
        let mut p = VideoPlayer::new(dec, (60, 1), false);
        assert!(
            p.preroll(std::time::Duration::from_secs(20)),
            "no first frame"
        );

        // 30p media on a 60p output: each media frame shows for exactly two
        // output frames, and the picture must actually change on the second.
        let a = p.frame_for(0).expect("frame at 0").rgba.clone();
        let b = p.frame_for(1).expect("frame at 1").rgba.clone();
        assert_eq!(a, b, "output frame 1 is still media frame 0");
        assert_eq!(a.len(), spec.frame_bytes());

        // Give the decoder room to produce; this is a test, not a frame budget.
        let mut c = None;
        for _ in 0..400 {
            let f = p.frame_for(2).expect("frame at 2");
            if f.index == 1 {
                c = Some(f.rgba.clone());
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let c = c.expect("media frame 1 never arrived");
        assert_ne!(a, c, "output frame 2 must be a NEW media frame");

        // Premultiplied on the way in (ADR 0012). The invariant is that no
        // colour channel exceeds alpha — stated that way rather than against a
        // literal, because a 10-bit YUVA round trip moves a flat 128 to 129 and
        // an exact comparison would be testing the encoder, not the premultiply.
        assert!(
            c.chunks_exact(4)
                .all(|px| px[0] <= px[3] && px[1] <= px[3] && px[2] <= px[3]),
            "frames must arrive premultiplied: no channel may exceed alpha"
        );
        let alpha = c[3];
        assert!(
            (120..=136).contains(&alpha),
            "the fixture is half-transparent; got alpha {alpha}"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_file_that_is_not_media_is_refused_by_name() {
        if !have_ffmpeg() {
            eprintln!("skipped: no ffmpeg on PATH");
            return;
        }
        let dir = std::env::temp_dir().join(format!("pmg-video-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("notes.txt");
        std::fs::write(&path, b"this is not a clip").unwrap();
        assert!(matches!(
            probe(&path),
            Err(MediaError::Probe(_)) | Err(MediaError::NoVideoStream)
        ));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn frame_bytes_is_rgba() {
        let s = VideoSpec {
            codec: "prores".into(),
            width: 1920,
            height: 1080,
            fps_num: 60,
            fps_den: 1,
            duration_s: 1.0,
            has_alpha: true,
            frames: 60,
        };
        assert_eq!(s.frame_bytes(), 1920 * 1080 * 4);
    }
}
