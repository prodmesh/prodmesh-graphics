//! Downscale, encode, and ship the preview channel.
//!
//! Risk R7, spike S7. Transport: MJPEG in M2 to get pixels on screen, then
//! WebCodecs H.264 over WebSocket in M3. Never WebRTC — signalling, ICE, and an
//! SFU question at eight viewers is not worth it on a LAN.
//!
//! # Non-negotiables
//!
//! - Downscale **the same GPU texture program uses**. Not a second render —
//!   anything else can drift from what goes to air.
//! - **Asynchronous readback.** A blocking buffer map on the render thread is
//!   the classic way to turn a preview feature into dropped output frames.
//! - Lower-priority queue, hard budget. **If preview exceeds its budget, drop
//!   the preview, log it, keep the show on air.**
//! - **Preview never shares the control socket.**
//!
//! # M0 status
//!
//! Types only.

pub fn selftest() -> Result<(), String> {
    Ok(())
}
