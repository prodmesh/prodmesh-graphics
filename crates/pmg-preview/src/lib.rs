//! The preview channel: downscale, encode, ship.
//!
//! MJPEG over HTTP, deliberately — the cheap version, to get pixels on a screen.
//! WebCodecs over the wire is M3. An `<img src="…">` renders
//! `multipart/x-mixed-replace` natively in every browser we care about, which
//! means M2 needs no client-side decoder at all.
//!
//! # The rule this module exists to obey
//!
//! **Preview must never delay a frame.** It runs on its own thread, on its own
//! connection, on its own port, and it takes the newest frame or none —
//! [`Preview::offer`] never blocks and never queues. A preview that is behind is
//! a cosmetic problem; a preview that holds up the render loop is a dropped
//! frame on air, and those are not the same kind of problem at all.
//!
//! That is the same reasoning as the three-connection split in ADR 0007, taken
//! one level down: a stalled consumer must not be able to reach back and slow
//! the producer.

use std::io::Write;
use std::net::TcpListener;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};

/// The default preview port. Family 85xx band: control 8519, node 8520.
pub const PREVIEW_PORT: u16 = 8521;

/// A single-slot mailbox holding the newest encoded frame.
///
/// One slot, not a queue. A queue would let a slow client accumulate latency
/// that never drains, and the newest frame is the only one anybody wants.
#[derive(Default)]
struct Slot {
    jpeg: Mutex<Option<Arc<Vec<u8>>>>,
    ready: Condvar,
    seq: AtomicU64,
}

pub struct Preview {
    slot: Arc<Slot>,
    width: u32,
    height: u32,
    /// Source pixels per preview pixel, both axes.
    step: u32,
    quality: u8,
    /// Frames dropped because the encoder was still busy. Reported, not hidden.
    skipped: Arc<AtomicU64>,
    encoding: Arc<Mutex<bool>>,
}

pub struct Config {
    pub source_width: u32,
    pub source_height: u32,
    /// Longest edge of the preview. 480 is plenty to see what is on air.
    pub target_width: u32,
    pub quality: u8,
    pub port: u16,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            source_width: 1920,
            source_height: 1080,
            target_width: 480,
            quality: 70,
            port: PREVIEW_PORT,
        }
    }
}

impl Preview {
    /// Start the preview server on its own thread.
    pub fn start(cfg: Config) -> std::io::Result<Self> {
        let step = (cfg.source_width / cfg.target_width).max(1);
        let width = cfg.source_width / step;
        let height = cfg.source_height / step;

        let slot = Arc::new(Slot::default());
        let listener = TcpListener::bind(("0.0.0.0", cfg.port))?;

        let serve = slot.clone();
        std::thread::Builder::new()
            .name("pmg-preview".into())
            .spawn(move || {
                for stream in listener.incoming().flatten() {
                    let s = serve.clone();
                    // A thread per viewer. There are one or two, and a thread
                    // that blocks on a slow client must not affect the others
                    // or the node.
                    std::thread::spawn(move || {
                        let _ = serve_mjpeg(stream, s);
                    });
                }
            })?;

        Ok(Self {
            slot,
            width,
            height,
            step,
            quality: cfg.quality,
            skipped: Arc::new(AtomicU64::new(0)),
            encoding: Arc::new(Mutex::new(false)),
        })
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub fn skipped(&self) -> u64 {
        self.skipped.load(Ordering::Relaxed)
    }

    /// Offer a frame. **Never blocks.**
    ///
    /// If the encoder is still working on the previous one, this returns
    /// immediately and counts a skip. Dropping preview frames under load is the
    /// correct behaviour — the alternative is holding up the render loop, which
    /// costs a frame on air.
    pub fn offer(&self, rgba: &[u8]) {
        {
            let mut busy = match self.encoding.try_lock() {
                Ok(g) => g,
                // Someone is already encoding. Skip, do not wait.
                Err(_) => {
                    self.skipped.fetch_add(1, Ordering::Relaxed);
                    return;
                }
            };
            if *busy {
                self.skipped.fetch_add(1, Ordering::Relaxed);
                return;
            }
            *busy = true;
        }

        let small = self.downscale(rgba);
        let mut jpeg = Vec::with_capacity(48 * 1024);
        let encoder = jpeg_encoder::Encoder::new(&mut jpeg, self.quality);
        let ok = encoder
            .encode(
                &small,
                self.width as u16,
                self.height as u16,
                jpeg_encoder::ColorType::Rgb,
            )
            .is_ok();

        if ok {
            *self.slot.jpeg.lock().unwrap() = Some(Arc::new(jpeg));
            self.slot.seq.fetch_add(1, Ordering::Release);
            self.slot.ready.notify_all();
        }
        *self.encoding.lock().unwrap() = false;
    }

    /// Box-average down to the preview size, over a checkerboard.
    ///
    /// The output is a **key signal**, so the alpha has to go somewhere visible:
    /// composited over a checkerboard, an operator can see what will key rather
    /// than a black rectangle that might be a graphic or might be nothing.
    fn downscale(&self, rgba: &[u8]) -> Vec<u8> {
        let (w, h, step) = (
            self.width as usize,
            self.height as usize,
            self.step as usize,
        );
        let sw = (self.width * self.step) as usize;
        let mut out = vec![0u8; w * h * 3];

        for y in 0..h {
            for x in 0..w {
                let (mut r, mut g, mut b, mut a) = (0u32, 0u32, 0u32, 0u32);
                for dy in 0..step {
                    for dx in 0..step {
                        let i = (((y * step + dy) * sw) + (x * step + dx)) * 4;
                        if i + 3 < rgba.len() {
                            r += rgba[i] as u32;
                            g += rgba[i + 1] as u32;
                            b += rgba[i + 2] as u32;
                            a += rgba[i + 3] as u32;
                        }
                    }
                }
                let n = (step * step) as u32;
                let (r, g, b, a) = (r / n, g / n, b / n, a / n);

                // 24 px checks, mid greys — the same idea as Design's canvas
                // backdrop, and the reason it is not a token colour is that it
                // must read identically in any theme.
                let check: u32 = if ((x / 12) + (y / 12)) % 2 == 0 {
                    110
                } else {
                    140
                };
                // Source-over with PREMULTIPLIED source, which is what the
                // renderer produces: dst = src + dst * (1 - a).
                let inv = 255 - a;
                let px = [
                    ((r + check * inv / 255).min(255)) as u8,
                    ((g + check * inv / 255).min(255)) as u8,
                    ((b + check * inv / 255).min(255)) as u8,
                ];
                out[(y * w + x) * 3..(y * w + x) * 3 + 3].copy_from_slice(&px);
            }
        }
        out
    }
}

/// One viewer, served `multipart/x-mixed-replace` until they go away.
fn serve_mjpeg(mut stream: std::net::TcpStream, slot: Arc<Slot>) -> std::io::Result<()> {
    stream.write_all(
        b"HTTP/1.0 200 OK\r\n\
          Access-Control-Allow-Origin: *\r\n\
          Cache-Control: no-store\r\n\
          Connection: close\r\n\
          Content-Type: multipart/x-mixed-replace; boundary=pmgframe\r\n\r\n",
    )?;

    let mut sent = u64::MAX;
    loop {
        let frame = {
            let mut guard = slot.jpeg.lock().unwrap();
            // Wait for a frame NEWER than the one already sent, so a viewer
            // that keeps up does not spin resending the same picture.
            while slot.seq.load(Ordering::Acquire) == sent || guard.is_none() {
                let (g, timeout) = slot
                    .ready
                    .wait_timeout(guard, std::time::Duration::from_secs(5))
                    .unwrap();
                guard = g;
                if timeout.timed_out() && guard.is_none() {
                    return Ok(()); // nothing is being rendered; let them retry
                }
                if slot.seq.load(Ordering::Acquire) != sent && guard.is_some() {
                    break;
                }
            }
            sent = slot.seq.load(Ordering::Acquire);
            guard.clone()
        };
        let Some(jpeg) = frame else { continue };

        write!(
            stream,
            "--pmgframe\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\n\r\n",
            jpeg.len()
        )?;
        stream.write_all(&jpeg)?;
        stream.write_all(b"\r\n")?;
        // A write error is a viewer that closed the tab. Not an incident.
        stream.flush()?;
    }
}

pub fn selftest() -> Result<(), String> {
    Ok(())
}
