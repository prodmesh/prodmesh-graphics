//! Mixing, resampling, and layer → output channel-pair mapping.
//!
//! Not in the pure set. Spec: `docs/specs/output-config.md` §7.
//!
//! The video output is the clock; this crate resamples to fit it. DeckLink
//! carries up to 16 channels, and which layer's audio lands on which pair is
//! part of output configuration rather than a template property — the same
//! template played on two outputs may need different routing.
//!
//! # M0 status
//!
//! Types only.

pub fn selftest() -> Result<(), String> {
    Ok(())
}
