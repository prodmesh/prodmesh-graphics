//! GPU display output via winit. Lands in M2.
//!
//! # The honest caveat
//!
//! A display output is **not phase-locked** to anything, absent an NVIDIA sync
//! card — and such a card locks to the display, not to SDI. A display output is
//! never placed in a phase-locked output group and is never shown as locked.
//! Configure says "free-running", in words (ADR 0011).
//!
//! # M0 status
//!
//! Types only.

pub fn selftest() -> Result<(), String> {
    Ok(())
}
