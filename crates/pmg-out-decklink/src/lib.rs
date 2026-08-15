//! DeckLink output backend. Lands in M2; spike S3 runs in M0.
//!
//! Safe Rust over `decklink-sys`. Risk R4.
//!
//! # Threading, which the SDK dictates
//!
//! The completion callback arrives on DeckLink's own thread. **Never allocate,
//! never block, never take a lock there** — push a token into a bounded channel
//! and return. Every interface pointer is wrapped in a type whose `Drop` calls
//! `Release`.
//!
//! # Key + fill
//!
//! The reliable path is **two ports on the same device, scheduled at the same
//! display time**. Same device means same clock, which is what actually
//! guarantees phase. Device capability varies wildly within a product line, so
//! **probing is mandatory** and Configure shows what the device reported rather
//! than what the model number suggests.
//!
//! # M0 status
//!
//! Types only. Spike S3 builds a standalone binary against `decklink-sys`
//! before any of this is written.

pub fn selftest() -> Result<(), String> {
    Ok(())
}
