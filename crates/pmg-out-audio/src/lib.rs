//! Audio out of a **host sound device** — CoreAudio, WASAPI, ALSA.
//!
//! The case this exists for: video on SDI, audio to a Dante Virtual Soundcard.
//! Until now audio only left this system embedded in SDI or NDI, so a plant
//! that takes its audio over Dante had no way in.
//!
//! # The video output is still the clock, and now there are two of them
//!
//! `pmg-audio` rests on one rule: audio has no rate of its own, it has a number
//! of samples that must accompany each video frame. That holds inside one
//! device, where the SDI card clocks both.
//!
//! A sound device has **its own crystal**. With Dante it is a genuinely
//! independent one — DVS is clocked by the network's PTP master, a DeckLink by
//! its reference input — so unless the house locks both to one master, the two
//! *will* walk apart. This is not a software defect to be fixed; it is two
//! clocks, and the only question is whether the drift is corrected or
//! discovered at the end of a service.
//!
//! So the same closed loop the DeckLink path uses applies here, against this
//! ring instead of the card's queue: the video clock says how many samples this
//! frame nominally carries, [`Sink::buffered`] says what the device still
//! holds, and `pmg_audio::AudioSync` picks the count actually sent. The
//! correction is one sample a frame — inaudible — and the node reports how hard
//! it is working, because a node quietly correcting all service is a node with
//! something else wrong.
//!
//! # Which host APIs
//!
//! Whatever cpal offers on the platform: CoreAudio on macOS, WASAPI on Windows,
//! ALSA on Linux, and ASIO behind a feature. **MME and DirectSound are not
//! here** — they are legacy paths to the same Windows device that WASAPI
//! reaches, and Dante Virtual Soundcard presents on all of them. The host is a
//! named, enumerated thing in this API precisely so adding one is additive.

mod ring;

use anyhow::{bail, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

pub use ring::Ring;

/// One host audio API this build can reach.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostInfo {
    /// Stable across runs and platforms — what a config file stores.
    pub id: String,
    pub name: String,
    pub default: bool,
}

/// One output device, as the host reports it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DeviceInfo {
    pub host: String,
    pub name: String,
    /// The most channels this device will open. DVS is commonly 8, 16 or 64.
    pub channels: u16,
    /// Whether it will run at the rate the mixer produces. A device that will
    /// not is listed anyway and refused at open, by name — an operator looking
    /// for a card they can see in the OS must not simply find it absent.
    pub supports_48k: bool,
    pub default: bool,
}

fn host_ids() -> Vec<cpal::HostId> {
    cpal::available_hosts()
}

pub fn hosts() -> Vec<HostInfo> {
    let default = cpal::default_host().id();
    host_ids()
        .into_iter()
        .map(|id| HostInfo {
            id: id.name().to_string(),
            name: id.name().to_string(),
            default: id == default,
        })
        .collect()
}

/// Every output device on every host this build can reach.
///
/// Enumeration is best effort per host: a host that will not answer is logged
/// and skipped rather than losing the others. On a machine with a broken
/// virtual driver, the working devices should still be offered.
pub fn devices() -> Vec<DeviceInfo> {
    let mut out = Vec::new();
    for id in host_ids() {
        let host = match cpal::host_from_id(id) {
            Ok(h) => h,
            Err(e) => {
                tracing::warn!(host = id.name(), "audio host would not open: {e}");
                continue;
            }
        };
        let default_name = host.default_output_device().map(|d| d.to_string());
        let devices = match host.output_devices() {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!(host = id.name(), "audio host would not enumerate: {e}");
                continue;
            }
        };
        for d in devices {
            let name = d.to_string();
            let configs: Vec<_> = d.supported_output_configs().map(|c| c.collect()).unwrap_or_default();
            let channels = configs.iter().map(|c| c.channels()).max().unwrap_or(0);
            let supports_48k = configs.iter().any(|c| {
                c.min_sample_rate() <= pmg_rate() && pmg_rate() <= c.max_sample_rate()
            });
            out.push(DeviceInfo {
                host: id.name().to_string(),
                default: Some(&name) == default_name.as_ref(),
                name,
                channels,
                supports_48k,
            });
        }
    }
    out
}

/// The one rate this system produces. `pmg_audio::SAMPLE_RATE` without taking a
/// dependency on it — this crate is a transport and should not need the mixer.
const fn pmg_rate() -> u32 {
    48_000
}

#[derive(Debug, Clone)]
pub struct Config {
    /// Host API name, or None for the platform default.
    pub host: Option<String>,
    /// Device name, or None for the host's default output.
    pub device: Option<String>,
    pub channels: u16,
    /// How many sample frames to hold. Two to three video frames is the natural
    /// depth: enough that a hiccup does not underrun into a click, shallow
    /// enough that the audio is not needlessly late.
    pub target_frames: u32,
}

/// An open host audio output.
///
/// Dropping it stops the stream. The device stays open for as long as this
/// lives, which is what makes [`buffered`](Self::buffered) meaningful.
pub struct Sink {
    ring: Arc<Ring>,
    channels: u16,
    primed: Arc<AtomicBool>,
    underruns: Arc<AtomicU64>,
    dropped: AtomicU64,
    device_name: String,
    host_name: String,
    // Held to keep the stream alive; cpal stops it on drop.
    _stream: cpal::Stream,
}

impl Sink {
    pub fn open(cfg: &Config) -> Result<Self> {
        let host = match &cfg.host {
            None => cpal::default_host(),
            Some(want) => {
                let id = host_ids()
                    .into_iter()
                    .find(|h| h.name().eq_ignore_ascii_case(want))
                    .with_context(|| {
                        let have: Vec<_> = hosts().into_iter().map(|h| h.name).collect();
                        format!("This machine has no audio host called “{want}”. It has: {}", have.join(", "))
                    })?;
                cpal::host_from_id(id).context("that audio host would not open")?
            }
        };
        let host_name = host.id().name().to_string();

        let device = match &cfg.device {
            None => host.default_output_device().context("this machine has no default audio output")?,
            Some(want) => host
                .output_devices()
                .context("that audio host would not enumerate its devices")?
                .find(|d| d.to_string() == *want)
                .with_context(|| {
                    format!("“{want}” is not an audio output on this machine. Check it is switched on, then look again.")
                })?,
        };
        let device_name = device.to_string();

        if cfg.channels == 0 {
            bail!("an audio output needs at least one channel");
        }
        // Pick a format the device offers at OUR rate and channel count. The
        // mixer's rate is not negotiable — everything in pmg-audio derives the
        // per-frame sample count from it — so a device that will not run at
        // 48 kHz is refused by name rather than resampled silently.
        let supported = device
            .supported_output_configs()
            .context("that device would not describe itself")?
            .find(|c| {
                c.channels() >= cfg.channels
                    && c.min_sample_rate() <= pmg_rate()
                    && pmg_rate() <= c.max_sample_rate()
            })
            .with_context(|| {
                format!(
                    "“{device_name}” will not run {} channels at 48 kHz. Check its own control panel.",
                    cfg.channels
                )
            })?;
        let format = supported.sample_format();
        let stream_channels = supported.channels();
        let stream_cfg = cpal::StreamConfig {
            channels: stream_channels,
            sample_rate: pmg_rate(),
            buffer_size: cpal::BufferSize::Default,
        };

        // Four video frames' worth, so a scheduling hiccup on either side has
        // somewhere to go. Never larger, because every sample in here is
        // latency the plant hears.
        let ring = Arc::new(Ring::new(
            (cfg.target_frames.max(1) as usize * 4) * stream_channels as usize,
        ));
        let underruns = Arc::new(AtomicU64::new(0));
        let primed = Arc::new(AtomicBool::new(false));

        let mix_channels = cfg.channels as usize;
        let dev_channels = stream_channels as usize;
        let stream = build_stream(
            &device,
            stream_cfg,
            format,
            Wiring {
                ring: Arc::clone(&ring),
                underruns: Arc::clone(&underruns),
                primed: Arc::clone(&primed),
                target_samples: cfg.target_frames.max(1) as usize * mix_channels,
                mix_channels,
                dev_channels,
            },
        )?;
        stream.play().context("that device would not start")?;

        tracing::info!(
            host = %host_name, device = %device_name,
            channels = mix_channels, opened_as = dev_channels, ?format,
            "audio output open",
        );
        Ok(Self {
            ring,
            channels: cfg.channels,
            primed,
            underruns,
            dropped: AtomicU64::new(0),
            device_name,
            host_name,
            _stream: stream,
        })
    }

    /// Hand this frame's interleaved samples to the device.
    ///
    /// Never blocks. A full ring means the device is consuming slower than the
    /// video clock is producing, which `AudioSync` is there to correct; the
    /// overflow is counted and reported rather than stalling the render thread.
    pub fn push(&self, interleaved: &[i32]) {
        let dropped = self.ring.push(interleaved);
        if dropped > 0 {
            self.dropped.fetch_add(dropped as u64, Ordering::Relaxed);
        }
    }

    /// Sample FRAMES the device still holds — the number `AudioSync` corrects
    /// against, exactly as a DeckLink's own queue depth is used.
    pub fn buffered(&self) -> u32 {
        (self.ring.len() / self.channels.max(1) as usize) as u32
    }

    /// Callbacks that ran short. Nonzero is audible, so it is reported and
    /// never smoothed away.
    pub fn underruns(&self) -> u64 {
        self.underruns.load(Ordering::Relaxed)
    }

    /// Samples that would not fit. The other side of the same story.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Whether the device has started draining.
    ///
    /// False until the ring first reaches its target depth. See the note on
    /// priming in [`build_stream`]: a buffer only reaches its working depth if
    /// something lets it fill first.
    pub fn primed(&self) -> bool {
        self.primed.load(Ordering::Relaxed)
    }

    pub fn device_name(&self) -> &str {
        &self.device_name
    }

    pub fn host_name(&self) -> &str {
        &self.host_name
    }
}

/// Build the output stream for whichever sample format the device wants.
///
/// The conversion from the mixer's i32 happens HERE, in the callback, because
/// the alternative is converting a whole frame on the render thread for a
/// device that may want something else again. It is a multiply per sample.
/// What the callback needs to do its job, in one piece.
///
/// The pieces travel together because they describe ONE stream — splitting them
/// into nine arguments made it possible to pass the mix width where the device
/// width belonged, which is silence in half the channels and no compiler error.
struct Wiring {
    ring: Arc<Ring>,
    underruns: Arc<AtomicU64>,
    primed: Arc<AtomicBool>,
    /// The ring depth that has to be reached before the device starts draining.
    target_samples: usize,
    /// Channels the MIXER produces.
    mix_channels: usize,
    /// Channels the DEVICE was opened with. Anything above `mix_channels` gets
    /// silence rather than a repeat of channel 1.
    dev_channels: usize,
}

fn build_stream(
    device: &cpal::Device,
    cfg: cpal::StreamConfig,
    format: cpal::SampleFormat,
    w: Wiring,
) -> Result<cpal::Stream> {
    let Wiring { ring, underruns, primed, target_samples, mix_channels, dev_channels } = w;
    // A device opened wider than the mix — 64-channel DVS fed 2 — gets silence
    // in the channels nobody addressed, rather than a repeat of channel 1.
    let err = |e| tracing::error!("audio device error: {e}");

    macro_rules! stream {
        ($sample:ty, $convert:expr) => {{
            let mut scratch: Vec<i32> = Vec::new();
            device.build_output_stream(
                cfg.clone(),
                move |out: &mut [$sample], _| {
                    // PRIMING. The device begins asking for samples the instant
                    // the stream starts, and the render thread cannot possibly
                    // be ahead of it yet — so draining from the first callback
                    // means running at a depth of nothing and clicking on every
                    // scheduling wobble. Measured: 268 underruns in two minutes,
                    // with the buffer never climbing above a couple of
                    // milliseconds.
                    //
                    // A DeckLink solves this with preroll: schedule N frames,
                    // THEN start. cpal has no preroll, so the callback does it —
                    // output silence until the ring first reaches its working
                    // depth. Those callbacks are not underruns; nothing was
                    // late, the buffer was filling.
                    if !primed.load(Ordering::Relaxed) {
                        if ring.len() < target_samples {
                            for s in out.iter_mut() {
                                *s = $convert(0i32);
                            }
                            return;
                        }
                        primed.store(true, Ordering::Relaxed);
                    }
                    let frames = out.len() / dev_channels.max(1);
                    scratch.clear();
                    scratch.resize(frames * mix_channels, 0);
                    let missing = ring.pop_into(&mut scratch);
                    if missing > 0 {
                        underruns.fetch_add(1, Ordering::Relaxed);
                    }
                    for f in 0..frames {
                        for c in 0..dev_channels {
                            let s = if c < mix_channels {
                                scratch[f * mix_channels + c]
                            } else {
                                0
                            };
                            out[f * dev_channels + c] = $convert(s);
                        }
                    }
                },
                err,
                None,
            )
        }};
    }

    let stream = match format {
        cpal::SampleFormat::F32 => stream!(f32, |s: i32| s as f32 / -(i32::MIN as f32)),
        cpal::SampleFormat::I16 => stream!(i16, |s: i32| (s >> 16) as i16),
        cpal::SampleFormat::I32 => stream!(i32, |s: i32| s),
        other => bail!("this device wants {other} samples, which this build cannot produce"),
    };
    stream.context("that device would not open an output stream")
}

/// `pmgctl --selftest`: the ring, without touching a sound device.
///
/// Opening a device in a selftest would make the gate depend on what is plugged
/// in, which is the opposite of what a selftest is for.
pub fn selftest() -> Result<(), String> {
    let r = Ring::new(64);
    if r.push(&[1, 2, 3]) != 0 {
        return Err("ring refused a push into an empty ring".into());
    }
    let mut out = [0i32; 4];
    if r.pop_into(&mut out) != 1 {
        return Err("ring did not report the missing sample".into());
    }
    if out[..3] != [1, 2, 3] {
        return Err("ring returned samples out of order".into());
    }
    Ok(())
}
