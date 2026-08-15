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

/// An open DeckLink output, optionally with a paired key port.
pub struct Sender {
    fill: decklink_sys::Output,
    /// The key half of a fill+key pair, when external keying is in use.
    key: Option<decklink_sys::Output>,
    width: u32,
    height: u32,
    fps_num: i32,
    fps_den: i32,
    /// Reused between frames so a service does not allocate two buffers a frame.
    fill_buf: std::cell::RefCell<Vec<u8>>,
    key_buf: std::cell::RefCell<Vec<u8>>,
}

pub struct Config {
    pub device_index: usize,
    /// Second device index for the key half. `None` means a single port.
    pub key_device_index: Option<usize>,
    pub mode: String,
    pub key_mode: KeyMode,
    pub preroll: i32,
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

        let fill = decklink_sys::Output::open(&dev, mode, cfg.key_mode, cfg.preroll)
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

        // The device handle must outlive the output it opened.
        std::mem::forget(dev);
        let (w, h) = (mode.width as u32, mode.height as u32);
        Ok(Self {
            fill,
            key,
            width: w,
            height: h,
            fps_num: mode.fps_num,
            fps_den: mode.fps_den,
            fill_buf: std::cell::RefCell::new(vec![0u8; (w * h * 4) as usize]),
            key_buf: std::cell::RefCell::new(vec![0u8; (w * h * 4) as usize]),
        })
    }

    /// Schedule one frame of **premultiplied** RGBA — what the renderer makes.
    ///
    /// Unpremultiplies here, at the boundary, and splits into fill and key when
    /// a pair is open.
    pub fn send_rgba_premultiplied(&self, rgba: &[u8]) -> Result<()> {
        let n = (self.width * self.height) as usize;
        if rgba.len() < n * 4 {
            bail!("frame is {} bytes, expected {}", rgba.len(), n * 4);
        }
        let mut fill = self.fill_buf.borrow_mut();
        let paired = self.key.is_some();
        let mut key = self.key_buf.borrow_mut();

        // The two conventions differ, and the spec is explicit about which is
        // which (`docs/specs/color-and-key.md` §5):
        //
        //   embedded alpha (one port) → STRAIGHT
        //   separate fill + key       → fill PREMULTIPLIED OVER BLACK, key = alpha
        //
        // The first version of this unpremultiplied in both cases. That is
        // right for embedded alpha and wrong for a pair: a downstream keyer
        // computing `bg*(1-key) + fill` expects fill already multiplied by
        // coverage, so sending straight fill brightens every soft edge — the
        // bright-fringe half of the failure §5 describes. Nothing in the code
        // said which convention it was implementing, which is how it went
        // unnoticed.
        for i in 0..n {
            let a = rgba[i * 4 + 3];

            if paired {
                // Premultiplied over black IS the renderer's own output — the
                // buffer is already exactly this, so there is nothing to undo.
                fill[i * 4] = rgba[i * 4 + 2];
                fill[i * 4 + 1] = rgba[i * 4 + 1];
                fill[i * 4 + 2] = rgba[i * 4];
                // The card is not keying on this port; an accidental 0 could
                // let some downstream path treat the frame as transparent.
                fill[i * 4 + 3] = 255;

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
            match self.fill.schedule_bgra(&fill) {
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
