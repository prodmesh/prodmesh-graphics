//! DeckLink output backend. **The M2 output.**
//!
//! NDI proved the render path (M1) on a backend with no frame-timing risk.
//! This one adds the hardware: a scheduled clock, a preroll, and a card that
//! will tell you plainly when you miss.
//!
//! # Keying: the thing that is easiest to get subtly wrong
//!
//! **The key is not a colour.** Fill goes RGB→YCbCr through a 601/709/2020
//! matrix. The key is a monotone *alpha* signal: `Y = alpha` mapped to the
//! configured range, `Cb = Cr =` neutral, and it must never go through a colour
//! matrix. Putting the key through the fill's matrix makes soft edges subtly
//! wrong downstream — visible on a keyer as a dark or light fringe that nobody
//! can trace back (`docs/specs/color-and-key.md` §4).
//!
//! Here the card takes 8-bit BGRA and does the conversion itself, so what this
//! module must get right is *which buffer goes to which port*:
//!
//! - **One port** — a fill only, carrying its own alpha. What a downstream
//!   device does with that alpha is its business.
//! - **Two ports** — fill carries the colour with alpha forced opaque; key
//!   carries alpha replicated into R, G and B, also opaque. That second buffer
//!   is a greyscale picture of coverage, which is exactly what a downstream
//!   keyer wants.
//!
//! # Two independent ports, not the card's paired mode
//!
//! A DeckLink can link two connectors itself (`bmdVideoOutputDualStream3D`-style
//! key+fill, `KeyMode::FillKeyPair` plus a `paired_port`), and that is a
//! different mechanism from what this does. **We open two ordinary outputs and
//! drive both**, which needs no keying capability on the device and works on any
//! two SDI outs — including the sub-devices of a Quad 2 that report no external
//! keying at all. The cost is that the two are kept in step by scheduling them
//! at the same display time rather than by the hardware, which is why both
//! halves are scheduled in one call below and never independently.
//!
//! # Premultiplied in, straight out
//!
//! Everything internal is premultiplied (ADR 0012). The conversion happens
//! here, at the boundary, once — the same rule and the same direction as
//! `pmg-out-ndi`, and for the same reason: getting it backwards is invisible
//! over one background and obvious over the other.

use anyhow::{bail, Context, Result};

pub use decklink_sys::KeyMode;

/// How many quarter-frame backoffs before a full queue stops being backpressure
/// and starts being a fault. At 59.94 this is about two seconds.
const MAX_BACKOFFS: u32 = 480;

/// What one DeckLink connector reports about itself.
///
/// **Discovered, never configured.** A capability an operator typed in is one
/// that is wrong the day the card moves, and a DeckLink read through a
/// mismatched SDK reports defaults rather than errors — so the honest source is
/// a query, every boot (`docs/INTEGRATION-NOTES.md`).
#[derive(Debug, Clone, Default)]
pub struct DeviceCapability {
    pub index: u32,
    /// What the driver calls this connector, e.g. `DeckLink Quad (2)`.
    pub display_name: String,
    pub model_name: String,
    /// The physical CARD. Two connectors on one card share a clock (ADR 0011),
    /// which is what makes a fill+key pair legal; connectors on two cards are
    /// out of phase and it looks fine until it does not.
    pub card: i64,
    pub supports_key_fill: bool,
    pub has_genlock: bool,
    pub genlock_locked: bool,
    pub ref_format: Option<String>,
    pub audio_channels: u32,
    pub modes: Vec<String>,
}

/// Every DeckLink connector this machine has, with what each one can do.
///
/// Empty when the SDK is not linked (the stub build CI relies on), when no
/// driver is loaded, or when the card genuinely is not there. Those are
/// different situations for a person and the same one for this function; the
/// node logs which, and `pmgctl decklink list` explains each in full.
///
/// Opening a device to read its modes is why this must run BEFORE the node
/// opens its own outputs. A failure on one connector degrades that connector to
/// "no modes reported" rather than losing the whole list — a card that answers
/// seven of eight questions should still offer the seven.
pub fn probe() -> Vec<DeviceCapability> {
    let count = match decklink_sys::device_count() {
        Ok(n) => n,
        Err(e) => {
            tracing::info!("no DeckLink devices to report: {e}");
            return Vec::new();
        }
    };
    (0..count)
        .filter_map(|i| {
            let info = decklink_sys::device_info(i)
                .map_err(|e| tracing::warn!("DeckLink {i} would not describe itself: {e}"))
                .ok()?;
            let dev = decklink_sys::Device::open(i).ok();
            let reference = dev.as_ref().and_then(|d| d.reference());
            Some(DeviceCapability {
                index: i as u32,
                display_name: info.display_name,
                model_name: info.model_name,
                // Both ids encode the sub-device in their low bits; the card is
                // the high part. Measured on a Quad 2: 0x5005_00..03 and
                // 0x5007_00..03 are two cards of four connectors.
                card: info.topological_id >> 8,
                supports_key_fill: info.supports_key_fill,
                has_genlock: info.has_reference_input,
                genlock_locked: reference.as_ref().is_some_and(|r| r.locked),
                ref_format: reference.map(|r| r.format).filter(|f| !f.is_empty()),
                audio_channels: info.max_audio_channels.max(0) as u32,
                modes: dev
                    .map(|d| d.modes().into_iter().map(|m| m.name).collect())
                    .unwrap_or_default(),
            })
        })
        .collect()
}

/// An open DeckLink output, optionally with a paired key port.
pub struct Sender {
    fill: decklink_sys::Output,
    /// The key half of a fill+key pair, when external keying is in use.
    key: Option<decklink_sys::Output>,
    width: u32,
    height: u32,
    fps_num: i32,
    fps_den: i32,
    /// Kept alive for the life of the output: it owns the handle the outputs
    /// were opened from, and reference status is read through it.
    dev: decklink_sys::Device,
    /// What the downstream is expected to do with alpha.
    ///
    /// The frame convention follows THIS, not whether a key port happens to be
    /// open — see `send_rgba_premultiplied`.
    key_mode: KeyMode,
    /// Reused between frames so a service does not allocate two buffers a frame.
    fill_buf: std::cell::RefCell<Vec<u8>>,
    key_buf: std::cell::RefCell<Vec<u8>>,
}

/// Fill the output buffers from one premultiplied RGBA frame.
///
/// Free, and pure, so the convention can be tested without a card. It has been
/// wrong twice — once by unpremultiplying for a pair, once by unpremultiplying
/// for a fill-only port — and both times the code said nothing about which
/// convention it meant.
pub fn build_frames(rgba: &[u8], fill: &mut [u8], key: &mut [u8], paired: bool, straight: bool) {
    let n = (rgba.len() / 4).min(fill.len() / 4);
    for i in 0..n {
        let a = rgba[i * 4 + 3];

        if paired || !straight {
            // Premultiplied over black IS the renderer's own output — the
            // buffer is already exactly this, so there is nothing to undo.
            // For a fill-only port that is also what a monitor, a converter
            // or a non-keying switcher input should receive.
            fill[i * 4] = rgba[i * 4 + 2];
            fill[i * 4 + 1] = rgba[i * 4 + 1];
            fill[i * 4 + 2] = rgba[i * 4];
            // The card is not keying on this port; an accidental 0 could
            // let some downstream path treat the frame as transparent.
            fill[i * 4 + 3] = 255;

            if !paired {
                continue;
            }

            // The key is a monotone alpha signal: coverage replicated into
            // all three channels, opaque. Never the fill's colour, and
            // never through a matrix.
            key[i * 4] = a;
            key[i * 4 + 1] = a;
            key[i * 4 + 2] = a;
            key[i * 4 + 3] = 255;
        } else {
            // Embedded alpha: the card wants STRAIGHT, so undo the
            // premultiply here, at the boundary, once.
            let (r, g, b) = if a == 0 {
                // Fully transparent unpremultiplies to black rather than to
                // infinity, and stale colour here shows up as a halo the
                // moment a keyer softens the edge.
                (0, 0, 0)
            } else if a == 255 {
                (rgba[i * 4], rgba[i * 4 + 1], rgba[i * 4 + 2])
            } else {
                let inv = 255.0 / a as f32;
                (
                    (rgba[i * 4] as f32 * inv).min(255.0) as u8,
                    (rgba[i * 4 + 1] as f32 * inv).min(255.0) as u8,
                    (rgba[i * 4 + 2] as f32 * inv).min(255.0) as u8,
                )
            };
            fill[i * 4] = b;
            fill[i * 4 + 1] = g;
            fill[i * 4 + 2] = r;
            fill[i * 4 + 3] = a;
        }
    }
}

pub struct Config {
    pub device_index: usize,
    /// Second device index for the key half. `None` means a single port.
    pub key_device_index: Option<usize>,
    pub mode: String,
    pub key_mode: KeyMode,
    pub preroll: i32,
    /// Embedded audio channels: 2, 8 or 16, or 0 for none.
    ///
    /// 0 is the right setting for a lower-third key feed, which has nothing to
    /// say, and is not a degraded configuration.
    pub audio_channels: i32,
}

impl Sender {
    pub fn open(cfg: &Config) -> Result<Self> {
        let dev = decklink_sys::Device::open(cfg.device_index as i32)
            .with_context(|| format!("opening DeckLink device {}", cfg.device_index))?;
        let info = decklink_sys::device_info(cfg.device_index as i32)
            .with_context(|| format!("reading capabilities of device {}", cfg.device_index))?;

        let modes = dev.modes();
        let mode = modes
            .iter()
            .find(|m| m.name.eq_ignore_ascii_case(&cfg.mode))
            .with_context(|| {
                format!(
                    "no display mode called {:?}. This device offers: {}",
                    cfg.mode,
                    modes
                        .iter()
                        .map(|m| m.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })?;

        // The card's own paired mode needs a paired_port the shim does not pass
        // yet, and it needs the device to support key+fill. We do not use it —
        // see the note at the top — so it is refused rather than half-supported.
        if matches!(cfg.key_mode, KeyMode::FillKeyPair | KeyMode::Embedded) {
            bail!(
                "hardware key+fill pairing is not wired up. Give two device indexes instead: \
                 one for the fill and one for the key. ({} reports key+fill support = {})",
                info.display_name,
                info.supports_key_fill
            );
        }

        let fill = decklink_sys::Output::open_with_audio(
            &dev,
            mode,
            cfg.key_mode,
            cfg.preroll,
            cfg.audio_channels,
        )
        .context("opening the fill output")?;

        let key = match cfg.key_device_index {
            Some(i) => {
                let kdev = decklink_sys::Device::open(i as i32)
                    .with_context(|| format!("opening DeckLink device {i} for the key"))?;
                let out = decklink_sys::Output::open(&kdev, mode, KeyMode::FillOnly, cfg.preroll)
                    .context("opening the key output")?;
                // The device handle must outlive the output it opened.
                std::mem::forget(kdev);
                Some(out)
            }
            None => None,
        };

        let (w, h) = (mode.width as u32, mode.height as u32);
        Ok(Self {
            key_mode: cfg.key_mode,
            fill,
            key,
            width: w,
            height: h,
            fps_num: mode.fps_num,
            fps_den: mode.fps_den,
            dev,
            fill_buf: std::cell::RefCell::new(vec![0u8; (w * h * 4) as usize]),
            key_buf: std::cell::RefCell::new(vec![0u8; (w * h * 4) as usize]),
        })
    }

    /// Schedule one frame of **premultiplied** RGBA — what the renderer makes.
    ///
    /// Unpremultiplies here, at the boundary, and splits into fill and key when
    /// a pair is open.
    pub fn send_rgba_premultiplied(&self, rgba: &[u8]) -> Result<()> {
        self.send_rgba_premultiplied_with_audio(rgba, &[])
    }

    /// As above, plus the audio that belongs to **this** frame.
    ///
    /// `audio` is interleaved 32-bit signed at 48 kHz, and its length must be
    /// this frame's sample count from `pmg_audio::AudioClock` — not a round
    /// number. The card takes it without complaint either way and drifts out of
    /// lip sync over a service if it is wrong (see INTEGRATION-NOTES).
    ///
    /// Audio goes to the FILL port only. The key half of a pair carries a
    /// monotone alpha signal and has no business carrying sound; embedding it
    /// twice would put the same audio on two SDI feeds that a plant may well
    /// sum.
    pub fn send_rgba_premultiplied_with_audio(&self, rgba: &[u8], audio: &[i32]) -> Result<()> {
        let n = (self.width * self.height) as usize;
        if rgba.len() < n * 4 {
            bail!("frame is {} bytes, expected {}", rgba.len(), n * 4);
        }
        let mut fill = self.fill_buf.borrow_mut();
        let paired = self.key.is_some();
        let mut key = self.key_buf.borrow_mut();

        // Does anything downstream actually consume the alpha byte?
        //
        // **SDI does not carry alpha.** A plain output converts BGRA to YCbCr
        // and drops the fourth channel on the floor, so straight colour arrives
        // with its coverage thrown away: a 10%-covered edge of white type,
        // unpremultiplied to (255,255,255,26), lands on the wire as FULL WHITE.
        // Every antialiased edge becomes a one-pixel halo at maximum brightness
        // and the type reads fattened and hard-edged.
        //
        // This branched on `self.key.is_some()` — the presence of a second port
        // — which meant a FillOnly output silently took the Embedded path. It
        // was invisible over NDI, which does carry alpha, and looked like bad
        // antialiasing on SDI rather than like a missing composite.
        //
        // Embedded alpha is now opt-in: it is correct only when the card is in
        // a keying mode, or the receiver is a capture device that keeps the
        // fourth channel.
        let straight = matches!(self.key_mode, KeyMode::Embedded);
        build_frames(rgba, &mut fill, &mut key, paired, straight);

        // BUSY is BACKPRESSURE, not a failure.
        //
        // The card's scheduling queue is full, which means it has not returned a
        // frame yet — that is exactly how a scheduled output paces its producer,
        // and it is the mechanism this loop relies on instead of a timer of its
        // own. Treating it as an error (which the first version did) makes the
        // node exit on its very first frame with "DeckLink call failed with code
        // -5", which reads like a hardware fault and is not one.
        //
        // Backing off a quarter of a frame rather than spinning: a busy-wait
        // here steals CPU from the render thread it is pacing.
        let quarter = std::time::Duration::from_micros(
            (250_000.0 / (self.fps_num as f64 / self.fps_den as f64)) as u64,
        );
        for attempt in 0.. {
            match self.fill.schedule_bgra_with_audio(&fill, audio) {
                Ok(()) => break,
                Err(e) => {
                    if attempt >= MAX_BACKOFFS {
                        // Long past backpressure. Something is actually wrong.
                        return Err(anyhow::anyhow!(
                            "the DeckLink fill queue stayed full for {} attempts: {e}",
                            MAX_BACKOFFS
                        ));
                    }
                    std::thread::sleep(quarter);
                }
            }
        }
        if let Some(k) = &self.key {
            // Scheduled straight after the fill so both land on the same display
            // time. They are kept in step by us, not by the hardware, so any
            // delay inserted between these two calls is a delay between the fill
            // and its key.
            for attempt in 0.. {
                match k.schedule_bgra(&key) {
                    Ok(()) => break,
                    Err(e) => {
                        if attempt >= MAX_BACKOFFS {
                            return Err(anyhow::anyhow!(
                                "the DeckLink key queue stayed full for {} attempts: {e}",
                                MAX_BACKOFFS
                            ));
                        }
                        std::thread::sleep(quarter);
                    }
                }
            }
        }
        Ok(())
    }

    /// What the card says about its reference input, right now.
    ///
    /// Read live rather than cached at open: a reference can be unplugged
    /// mid-service, and a status that only reflects boot time is worse than
    /// none because it is confidently stale.
    ///
    /// Uses the device handle this Sender already holds. The first version
    /// re-opened the device by index and silently got `None` every time,
    /// because the device is already open — by us.
    pub fn reference(&self) -> Option<decklink_sys::Reference> {
        self.dev.reference()
    }

    pub fn stats(&self) -> decklink_sys::Stats {
        self.fill.stats()
    }

    pub fn key_stats(&self) -> Option<decklink_sys::Stats> {
        self.key.as_ref().map(|k| k.stats())
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }
}

pub fn selftest() -> Result<(), String> {
    // Nothing to check without hardware, and a node with no DeckLink installed
    // must still pass its own selftest — the absence is a deployment fact.
    Ok(())
}

#[cfg(test)]
mod probe_tests {
    use super::*;

    /// The stub build — no vendor SDK — must report an empty list rather than
    /// failing. That configuration is supported and CI relies on it; a probe
    /// that panicked without a card would take the node down on every machine
    /// that has none.
    #[test]
    fn probing_without_hardware_is_an_empty_list_not_a_failure() {
        let found = probe();
        for d in &found {
            // Whatever came back has to be usable as an identity: an unnamed
            // connector in a picker is a row nobody can choose between.
            assert!(!d.display_name.is_empty(), "a connector must name itself");
            assert!(
                d.audio_channels <= 64,
                "audio channels look like a default, not a report"
            );
        }
    }

    /// Connectors on ONE card share a clock (ADR 0011). Grouping is what makes
    /// a legal fill+key pair identifiable, and grouping on the wrong id invents
    /// eight single-connector "cards" — measured on a Quad 2, where
    /// `persistent_id` increments per connector and `topological_id` does not.
    #[test]
    fn connectors_are_grouped_by_card_not_by_connector() {
        let found = probe();
        if found.len() < 2 {
            return; // nothing to group; the machine has no multi-port card
        }
        let cards: std::collections::BTreeSet<i64> = found.iter().map(|d| d.card).collect();
        assert!(
            cards.len() < found.len(),
            "every connector reported its own card id — grouping is on the wrong field",
        );
    }
}
