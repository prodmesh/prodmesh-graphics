//! H.264 preview over a WebSocket, for `VideoDecoder` in the browser.
//!
//! # Why this rather than MJPEG, and why not WebRTC
//!
//! MJPEG (in [`crate`]) needs no client decoder at all, which is exactly why M2
//! used it. It also sends every frame whole: a 480x270 preview at 59.94 is
//! several megabits of independent JPEGs, and a 1080p one is not worth
//! discussing. H.264 sends the difference.
//!
//! WebRTC would be the obvious alternative and is the wrong tool here (R9): SDP,
//! ICE, and a TURN server, on a closed booth LAN with no TLS, to move a picture
//! one hop. WebCodecs takes an `EncodedVideoChunk` and a canvas, over the
//! WebSocket we would need anyway.
//!
//! # The rules
//!
//! - **Preview must never delay a frame.** [`H264Preview::offer`] is
//!   non-blocking: one slot, newest wins, skips counted. A stalled browser must
//!   not be able to reach back and slow the render loop (ADR 0007).
//! - **A late joiner must paint immediately.** The GOP is short and SPS/PPS are
//!   repeated before every keyframe, so a browser that connects mid-service
//!   waits for the next keyframe rather than for a stream restart.
//! - **The preview is a key signal**, so alpha is composited over a
//!   checkerboard before encoding — H.264 has no alpha to carry, and a black
//!   rectangle that might be a graphic or might be nothing is not a preview.
//!
//! # Encoding is a child process, for the same reason decoding is
//!
//! `h264_videotoolbox` is hardware and carries no GPL; `libx264` is GPL and
//! would contaminate a linked binary. Running ffmpeg as a separate process
//! keeps that a runtime choice rather than a licensing one — the same
//! reasoning as `pmg_media::video`, and the reason this can prefer hardware and
//! fall back without either option changing what this repo may be licensed as.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};

/// One encoded access unit, ready to become an `EncodedVideoChunk`.
#[derive(Clone)]
pub struct Au {
    pub bytes: Arc<Vec<u8>>,
    /// IDR. The browser cannot start on anything else.
    pub key: bool,
}

/// How many frames between keyframes.
///
/// Half a second at 59.94. Short because the number that matters is **how long
/// a browser stares at nothing after connecting**, not the bitrate — this is a
/// confidence monitor on a LAN, not a distribution encode.
const GOP: u32 = 30;

pub struct Config {
    pub width: u32,
    pub height: u32,
    pub fps_num: u32,
    pub fps_den: u32,
    pub port: u16,
    /// Target bits per second. 4 Mbit is generous for 480x270 and keeps the
    /// picture honest enough to see a soft key edge.
    pub bitrate: u32,
}

pub struct H264Preview {
    tx: SyncSender<Vec<u8>>,
    skipped: Arc<AtomicU64>,
    clients: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    child: Arc<Mutex<Option<Child>>>,
    hub: Arc<Hub>,
}

impl H264Preview {
    pub fn start(cfg: Config) -> anyhow::Result<Self> {
        let mut child = spawn_encoder(&cfg)?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow::anyhow!("encoder gave no stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("encoder gave no stdout"))?;

        // One slot. A queue here would buy latency the preview can never give
        // back, and the newest frame is the only one anyone wants.
        let (tx, rx) = sync_channel::<Vec<u8>>(1);
        let skipped = Arc::new(AtomicU64::new(0));
        let clients = Arc::new(AtomicU64::new(0));
        let stop = Arc::new(AtomicBool::new(false));

        // Feeder: render loop -> encoder stdin. On its own thread so a slow
        // encoder blocks here and not in `offer`.
        let feed_stop = stop.clone();
        std::thread::Builder::new()
            .name("pmg-preview-feed".into())
            .spawn(move || {
                while !feed_stop.load(Ordering::Relaxed) {
                    match rx.recv() {
                        Ok(frame) => {
                            if stdin.write_all(&frame).is_err() {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
            })?;

        // Fan-out: the newest access unit, plus the last keyframe so a joiner
        // has something to start on.
        let hub: Arc<Hub> = Arc::new(Hub::default());
        let read_hub = hub.clone();
        let read_stop = stop.clone();
        std::thread::Builder::new()
            .name("pmg-preview-encode".into())
            .spawn(move || {
                read_annex_b(stdout, |au| {
                    if read_stop.load(Ordering::Relaxed) {
                        return false;
                    }
                    read_hub.publish(au);
                    true
                });
            })?;

        let listener = TcpListener::bind(("0.0.0.0", cfg.port))?;
        let serve_hub = hub.clone();
        let serve_clients = clients.clone();
        std::thread::Builder::new()
            .name("pmg-preview-ws".into())
            .spawn(move || {
                for stream in listener.incoming().flatten() {
                    let h = serve_hub.clone();
                    let c = serve_clients.clone();
                    std::thread::spawn(move || {
                        c.fetch_add(1, Ordering::Relaxed);
                        let _ = serve_ws(stream, h);
                        c.fetch_sub(1, Ordering::Relaxed);
                    });
                }
            })?;

        Ok(Self {
            tx,
            skipped,
            clients,
            stop,
            child: Arc::new(Mutex::new(Some(child))),
            hub,
        })
    }

    /// Offer the newest audio levels as JSON bytes. Never blocks; every
    /// connected viewer gets the latest on its next loop turn.
    pub fn offer_levels(&self, json: Vec<u8>) {
        self.hub.publish_levels(json);
    }

    /// Offer a downscaled RGB frame. **Never blocks.**
    ///
    /// If the encoder has not taken the previous frame this returns immediately
    /// and counts a skip. Dropping preview frames under load is correct; the
    /// alternative costs a frame on air.
    pub fn offer(&self, rgb: Vec<u8>) {
        match self.tx.try_send(rgb) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
                self.skipped.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    pub fn skipped(&self) -> u64 {
        self.skipped.load(Ordering::Relaxed)
    }

    /// Attached browsers, so an output can say "nobody is watching".
    pub fn clients(&self) -> u64 {
        self.clients.load(Ordering::Relaxed)
    }
}

impl Drop for H264Preview {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Ok(mut c) = self.child.lock() {
            if let Some(mut ch) = c.take() {
                let _ = ch.kill();
                let _ = ch.wait();
            }
        }
    }
}

fn spawn_encoder(cfg: &Config) -> anyhow::Result<Child> {
    // Hardware first. VideoToolbox is not GPL and does not heat the machine
    // that is also rendering; libx264 is the fallback and is GPL, which is
    // survivable ONLY because this is a separate process.
    for codec in ["h264_videotoolbox", "libx264"] {
        let mut cmd = Command::new("ffmpeg");
        cmd.args(["-hide_banner", "-loglevel", "error", "-nostdin"]);
        cmd.args([
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgb24",
            "-s",
            &format!("{}x{}", cfg.width, cfg.height),
            "-r",
            &format!("{}/{}", cfg.fps_num, cfg.fps_den),
            "-i",
            "-",
            "-an",
            "-c:v",
            codec,
        ]);
        if codec == "libx264" {
            // zerolatency: no B-frames and no lookahead. A preview that is
            // right and 200 ms late is worse than one that is right and now.
            cmd.args(["-preset", "veryfast", "-tune", "zerolatency"]);
        } else {
            cmd.args(["-realtime", "1"]);
        }
        cmd.args([
            "-b:v",
            &format!("{}", cfg.bitrate),
            "-g",
            &format!("{GOP}"),
            "-pix_fmt",
            "yuv420p",
            // SPS/PPS before EVERY keyframe, not just the first. Without this a
            // browser that connects one second in has no parameter sets and
            // shows nothing at all, forever, with no error.
            "-bsf:v",
            "dump_extra=freq=keyframe",
            "-f",
            "h264",
            "-",
        ]);
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());

        if let Ok(child) = cmd.spawn() {
            tracing::info!(codec, "preview encoder");
            return Ok(child);
        }
    }
    anyhow::bail!("could not start an H.264 encoder; is ffmpeg on PATH?")
}

/// Split an Annex-B stream into access units and hand each to `sink`.
///
/// An access unit here is "everything from one AUD/SPS/IDR boundary to the
/// next" — the browser wants whole frames, not individual NALs.
fn read_annex_b(mut src: impl Read, mut sink: impl FnMut(Au) -> bool) {
    let mut buf: Vec<u8> = Vec::with_capacity(256 * 1024);
    let mut chunk = [0u8; 32 * 1024];
    let mut pending: Vec<u8> = Vec::new();
    let mut pending_key = false;

    loop {
        let n = match src.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        buf.extend_from_slice(&chunk[..n]);

        // Walk start codes. Everything before the LAST one is complete.
        let mut starts = start_codes(&buf);
        if starts.len() < 2 {
            continue;
        }
        let last = starts.pop().unwrap();
        for w in starts
            .windows(2)
            .chain(std::iter::once([starts[starts.len() - 1], last].as_slice()))
        {
            let (a, b) = (w[0], w[1]);
            let nal = &buf[a..b];
            let ty = nal_type(nal);
            // 9 = AUD, 7 = SPS, 5 = IDR, 1 = non-IDR slice.
            // A new access unit begins at an AUD, an SPS, or a slice that
            // follows a slice.
            let boundary = matches!(ty, Some(9) | Some(7)) && !pending.is_empty();
            if boundary {
                if !sink(Au {
                    bytes: Arc::new(std::mem::take(&mut pending)),
                    key: pending_key,
                }) {
                    return;
                }
                pending_key = false;
            }
            if ty == Some(5) {
                pending_key = true;
            }
            pending.extend_from_slice(nal);
            // A slice NAL ends its access unit.
            if matches!(ty, Some(1) | Some(5)) {
                if !sink(Au {
                    bytes: Arc::new(std::mem::take(&mut pending)),
                    key: pending_key,
                }) {
                    return;
                }
                pending_key = false;
            }
        }
        buf.drain(..last);
    }
}

/// Byte offsets of every Annex-B start code in `buf`.
fn start_codes(buf: &[u8]) -> Vec<usize> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 3 <= buf.len() {
        if buf[i] == 0 && buf[i + 1] == 0 && buf[i + 2] == 1 {
            // Prefer the 4-byte form so the NAL keeps its own prefix.
            let s = if i > 0 && buf[i - 1] == 0 { i - 1 } else { i };
            out.push(s);
            i += 3;
        } else {
            i += 1;
        }
    }
    out
}

/// `nal_unit_type` of the NAL beginning at a start code.
fn nal_type(nal: &[u8]) -> Option<u8> {
    let mut i = 0;
    while i + 3 <= nal.len() {
        if nal[i] == 0 && nal[i + 1] == 0 && nal[i + 2] == 1 {
            return nal.get(i + 3).map(|b| b & 0x1F);
        }
        i += 1;
    }
    None
}

#[derive(Default)]
struct Hub {
    inner: Mutex<HubState>,
    seq: AtomicU64,
    /// The newest audio-levels payload (JSON bytes), on its own sequence —
    /// levels tick at their own rate and must not wait for a video frame,
    /// nor make one wait.
    levels: Mutex<Option<Arc<Vec<u8>>>>,
    levels_seq: AtomicU64,
}

#[derive(Default)]
struct HubState {
    latest: Option<Au>,
    /// The most recent keyframe, kept so a joiner has a starting point instead
    /// of a wait of up to a whole GOP.
    last_key: Option<Au>,
}

impl Hub {
    fn publish(&self, au: Au) {
        let mut s = self.inner.lock().unwrap();
        if au.key {
            s.last_key = Some(au.clone());
        }
        s.latest = Some(au);
        self.seq.fetch_add(1, Ordering::Release);
    }

    fn snapshot(&self, since: u64) -> Option<(u64, Au)> {
        let seq = self.seq.load(Ordering::Acquire);
        if seq == since {
            return None;
        }
        let s = self.inner.lock().unwrap();
        s.latest.clone().map(|au| (seq, au))
    }

    fn last_key(&self) -> Option<Au> {
        self.inner.lock().unwrap().last_key.clone()
    }

    fn publish_levels(&self, json: Vec<u8>) {
        *self.levels.lock().unwrap() = Some(Arc::new(json));
        self.levels_seq.fetch_add(1, Ordering::Release);
    }

    fn levels_snapshot(&self, since: u64) -> Option<(u64, Arc<Vec<u8>>)> {
        let seq = self.levels_seq.load(Ordering::Acquire);
        if seq == since {
            return None;
        }
        self.levels.lock().unwrap().clone().map(|b| (seq, b))
    }
}

/// One browser, served WebSocket binary frames until it goes away.
///
/// Hand-rolled rather than pulling a server-side WebSocket stack in: the
/// handshake is a SHA-1 and a base64, the frames we send are binary and never
/// fragmented, and the alternative is a dependency in the process that must not
/// fail (ADR 0003's reasoning, one level down).
fn serve_ws(mut stream: TcpStream, hub: Arc<Hub>) -> std::io::Result<()> {
    let mut head = [0u8; 2048];
    let n = stream.read(&mut head)?;
    let req = String::from_utf8_lossy(&head[..n]).to_string();

    let key = req
        .lines()
        .find_map(|l| l.strip_prefix("Sec-WebSocket-Key: "))
        .map(|k| k.trim().to_string());

    let Some(key) = key else {
        // Not a WebSocket. Say so in a way a browser will show.
        stream.write_all(
            b"HTTP/1.1 400 Bad Request\r\nContent-Type: text/plain\r\n\
              Connection: close\r\n\r\nThis port speaks WebSocket H.264 preview.\n",
        )?;
        return Ok(());
    };

    let accept = ws_accept(&key);
    stream.write_all(
        format!(
            "HTTP/1.1 101 Switching Protocols\r\n\
             Upgrade: websocket\r\nConnection: Upgrade\r\n\
             Sec-WebSocket-Accept: {accept}\r\n\r\n"
        )
        .as_bytes(),
    )?;
    stream.set_nodelay(true)?;

    // Start on a keyframe or the browser decodes nothing and says nothing.
    if let Some(k) = hub.last_key() {
        write_frame(&mut stream, u8::from(k.key), &k.bytes)?;
    }

    let mut sent = u64::MAX;
    let mut sent_levels = u64::MAX;
    loop {
        let mut idle = true;
        if let Some((seq, au)) = hub.snapshot(sent) {
            sent = seq;
            write_frame(&mut stream, u8::from(au.key), &au.bytes)?;
            idle = false;
        }
        if let Some((seq, bytes)) = hub.levels_snapshot(sent_levels) {
            sent_levels = seq;
            write_frame(&mut stream, FRAME_LEVELS, &bytes)?;
            idle = false;
        }
        if idle {
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }
}

/// The type byte prefixed to every binary frame: 0 = delta access unit,
/// 1 = key access unit, 2 = audio levels as JSON. One byte, because the
/// hand-rolled server has exactly one frame shape and the byte is cheaper
/// than a second framing scheme.
pub const FRAME_LEVELS: u8 = 2;

/// One payload as a WebSocket binary frame, with its one-byte type header.
fn write_frame(stream: &mut TcpStream, kind: u8, bytes: &[u8]) -> std::io::Result<()> {
    let len = bytes.len() + 1;
    let mut hdr = Vec::with_capacity(10);
    hdr.push(0x82); // FIN | binary
    if len < 126 {
        hdr.push(len as u8);
    } else if len <= u16::MAX as usize {
        hdr.push(126);
        hdr.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        hdr.push(127);
        hdr.extend_from_slice(&(len as u64).to_be_bytes());
    }
    stream.write_all(&hdr)?;
    stream.write_all(&[kind])?;
    stream.write_all(bytes)?;
    stream.flush()
}

/// `base64(sha1(key + GUID))`, from RFC 6455.
fn ws_accept(key: &str) -> String {
    const GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
    base64(&sha1(format!("{key}{GUID}").as_bytes()))
}

fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0];
    let ml = (data.len() as u64) * 8;
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&ml.to_be_bytes());

    for block in msg.chunks_exact(64) {
        let mut w = [0u32; 80];
        for (i, c) in block.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([c[0], c[1], c[2], c[3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let (mut a, mut b, mut c, mut d, mut e) = (h[0], h[1], h[2], h[3], h[4]);
        for (i, wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5A827999u32),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDC),
                _ => (b ^ c ^ d, 0xCA62C1D6),
            };
            let t = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = t;
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }

    let mut out = [0u8; 20];
    for (i, v) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&v.to_be_bytes());
    }
    out
}

fn base64(data: &[u8]) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in data.chunks(3) {
        let b = [c[0], *c.get(1).unwrap_or(&0), *c.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(T[(n >> 18 & 63) as usize] as char);
        out.push(T[(n >> 12 & 63) as usize] as char);
        out.push(if c.len() > 1 {
            T[(n >> 6 & 63) as usize] as char
        } else {
            '='
        });
        out.push(if c.len() > 2 {
            T[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The one vector every WebSocket implementation is checked against
    /// (RFC 6455 §1.3). Getting this wrong is a handshake a browser refuses
    /// with no useful message.
    #[test]
    fn the_rfc_6455_handshake_vector() {
        assert_eq!(
            ws_accept("dGhlIHNhbXBsZSBub25jZQ=="),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
    }

    #[test]
    fn sha1_matches_the_known_digests() {
        assert_eq!(
            sha1(b"abc").to_vec(),
            hex(b"a9993e364706816aba3e25717850c26c9cd0d89d")
        );
        assert_eq!(
            sha1(b"").to_vec(),
            hex(b"da39a3ee5e6b4b0d3255bfef95601890afd80709")
        );
    }

    fn hex(s: &[u8]) -> Vec<u8> {
        s.chunks(2)
            .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap())
            .collect()
    }

    #[test]
    fn base64_pads_like_everyone_else() {
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
    }

    #[test]
    fn nal_types_come_off_either_start_code_length() {
        assert_eq!(nal_type(&[0, 0, 1, 0x65]), Some(5)); // IDR
        assert_eq!(nal_type(&[0, 0, 0, 1, 0x67]), Some(7)); // SPS
        assert_eq!(nal_type(&[0, 0, 0, 1, 0x41]), Some(1)); // non-IDR slice
        assert_eq!(nal_type(&[1, 2, 3]), None);
    }

    #[test]
    fn start_codes_prefer_the_four_byte_form() {
        // A 4-byte start code must not be reported as a 3-byte one starting a
        // byte later, or every NAL keeps a stray zero on its front.
        let buf = [0u8, 0, 0, 1, 0x67, 9, 0, 0, 0, 1, 0x65];
        assert_eq!(start_codes(&buf), vec![0, 6]);
    }
}
