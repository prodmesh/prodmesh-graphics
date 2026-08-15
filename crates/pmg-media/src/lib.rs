//! Image and video demux/decode, frame cache, and the A/V clock.
//!
//! Not in the pure set. Risk R5, spike S5.
//!
//! # Two rules
//!
//! **The clock is always the video output.** Audio resamples to fit. The
//! decoder is *pulled* by the output callback, never pushed. Any design where
//! the decoder drives timing is wrong by construction.
//!
//! **On decoder underrun: hold the last frame, log it, never stall the
//! output.** A held frame is a glitch; a stalled output is a black picture.
//!
//! # Codec policy
//!
//! Cued media must be an all-intra intermediate — ProRes 4444, DNxHR, or HAP
//! with alpha — because long-GOP H.264 with B-frames is where frame-accurate
//! seek goes to die. The control server transcodes on ingest
//! (`docs/specs/asset-cas.md` §3). H.264 is accepted for convenience playback.
//!
//! Variable-frame-rate sources are **rejected at ingest**, not accommodated.
//!
//! # Licensing
//!
//! ffmpeg **LGPL components only, dynamically linked, no GPL**. Linking
//! `libx264` for convenience would quietly make the whole binary GPL and
//! foreclose the posture in `docs/VISION.md` permanently (ADR 0003 §Seam).
//!
//! # M0 status
//!
//! Types only.

pub fn selftest() -> Result<(), String> {
    Ok(())
}
