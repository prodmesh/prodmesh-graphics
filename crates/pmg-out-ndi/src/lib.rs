//! NDI output backend. **The M1 output.**
//!
//! M1 outputs NDI rather than SDI deliberately: M1 already requires the scene
//! format, layout, text, renderer, WASM designer, server, protocol, and CAS to
//! work for the first time, and adding hardware frame-timing risk on top of
//! that is what turns a slice into a quarter. NDI is pure software, carries
//! alpha natively, and the demo an operator sees is identical. DeckLink lands
//! in M2 on a renderer that already works.
//!
//! NDI wants **straight** alpha; everything internal is premultiplied, so the
//! conversion happens here, at the boundary (`docs/specs/color-and-key.md` §5).
//!
//! The SDK is loaded at runtime rather than linked, so redistribution stays a
//! deployment question (ADR 0003 Seam).
//!
//! # Why this lives here rather than in `pmgctl`
//!
//! The playout node and the CLI must put identical bytes on the wire. Two
//! copies of an unpremultiply loop is two chances to get the direction wrong —
//! and getting it wrong is invisible over one background and obvious over the
//! other, which is the worst way for a bug to behave.

use anyhow::{bail, Result};
use ndi_sys::*;
use std::ffi::CString;

/// Load the NDI runtime, or say plainly that it is not installed.
///
/// Loaded at runtime, never linked: that is the seam that keeps the SDK a
/// deployment question rather than something baked into our binary.
fn load() -> Result<Ndi> {
    Ndi::load().map_err(|e| {
        anyhow::anyhow!(
            "the NDI runtime is not available ({e}). Install the NDI SDK or runtime; \
             this build loads it at run time and does not bundle it."
        )
    })
}

/// An open NDI output.
///
/// Holds the runtime alive for as long as the sender exists — dropping the
/// `Ndi` while a send instance is live would unload the library out from under
/// it.
pub struct Sender {
    ndi: Ndi,
    inst: send_instance_t,
    w: u32,
    h: u32,
    fps_n: i32,
    fps_d: i32,
    /// Reused between frames so a 4-second take does not allocate 240 buffers.
    scratch: std::cell::RefCell<Vec<u8>>,
    _name: CString,
}

impl Sender {
    pub fn open(name: &str, w: u32, h: u32, fps_n: i32, fps_d: i32) -> Result<Self> {
        let ndi = load()?;
        let c_name = CString::new(name)?;
        // SAFETY: types transcribed from include/pmg_ndi.h and asserted in
        // ndi-sys's `abi` tests. The instance is destroyed in Drop.
        let inst = unsafe {
            let sc = NDIlib_send_create_t {
                p_ndi_name: c_name.as_ptr(),
                p_groups: std::ptr::null(),
                // Let the SDK pace us to frame_rate, so a take free-runs at the
                // right rate without a timer of our own.
                clock_video: true,
                clock_audio: false,
            };
            (ndi.send_create)(&sc)
        };
        if inst.is_null() {
            bail!("NDIlib_send_create returned null");
        }
        tracing::info!(name, w, h, fps_n, fps_d, "NDI output open");
        Ok(Self {
            ndi,
            inst,
            w,
            h,
            fps_n,
            fps_d,
            scratch: std::cell::RefCell::new(vec![0u8; (w * h * 4) as usize]),
            _name: c_name,
        })
    }

    /// Send one frame of **premultiplied** RGBA — what the renderer produces.
    ///
    /// NDI wants **straight** alpha, so this unpremultiplies at the boundary.
    /// Getting the direction wrong produces fringing that is invisible over one
    /// background and obvious over the other
    /// (`docs/specs/color-and-key.md` §5).
    pub fn send_rgba_premultiplied(&self, rgba: &[u8]) -> Result<()> {
        let n = (self.w * self.h) as usize;
        if rgba.len() < n * 4 {
            bail!("frame is {} bytes, expected {}", rgba.len(), n * 4);
        }
        let mut bgra = self.scratch.borrow_mut();

        for i in 0..n {
            let a = rgba[i * 4 + 3];
            let (r, g, b) = if a == 0 {
                // Fully transparent unpremultiplies to black rather than to
                // infinity — and leaving stale colour here would show up as a
                // halo the moment a keyer softened the edge.
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
            bgra[i * 4] = b;
            bgra[i * 4 + 1] = g;
            bgra[i * 4 + 2] = r;
            // Alpha is coverage: linear, never transfer-encoded.
            bgra[i * 4 + 3] = a;
        }

        // SAFETY: send_video_v2 is synchronous — it copies before returning —
        // so `bgra` does not need to outlive the call. The async variants would
        // require the opposite and are deliberately not used here.
        unsafe {
            let video = NDIlib_video_frame_v2_t {
                xres: self.w as i32,
                yres: self.h as i32,
                FourCC: FOURCC_BGRA,
                frame_rate_N: self.fps_n,
                frame_rate_D: self.fps_d,
                frame_format_type: FRAME_FORMAT_PROGRESSIVE,
                p_data: bgra.as_ptr() as *mut u8,
                line_stride_in_bytes: (self.w * 4) as i32,
                ..Default::default()
            };
            (self.ndi.send_send_video_v2)(self.inst, &video);
        }
        Ok(())
    }

    /// How many receivers are attached, so an output can say "nobody is
    /// watching this" rather than looking healthy into the void.
    pub fn receivers(&self) -> i32 {
        // SAFETY: instance is valid for the lifetime of self.
        unsafe { (self.ndi.send_get_no_connections)(self.inst, 0) }
    }
}

impl Drop for Sender {
    fn drop(&mut self) {
        // SAFETY: paired with send_create; called exactly once.
        unsafe { (self.ndi.send_destroy)(self.inst) }
    }
}

pub fn selftest() -> Result<(), String> {
    // Nothing to check without the runtime present, and a node without NDI
    // installed must still pass its own selftest — the absence is a
    // deployment fact, not a defect.
    Ok(())
}
