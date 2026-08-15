//! `wasm-bindgen` surface for Design mode.
//!
//! The browser-side half of ADR 0001: compile a template, solve layout, shape
//! text, and render to a canvas — using exactly the same crates the playout
//! node uses. That is what eliminates the entire class of "it wrapped
//! differently in the designer" bugs.
//!
//! # Constraints that are the situation, not a simplification
//!
//! - **Single-threaded.** `SharedArrayBuffer` and WASM threads need COOP/COEP,
//!   which conflicts with a plain-HTTP LAN appliance that deliberately has no
//!   TLS. Size the work accordingly: half-resolution preview at 30 fps is fine;
//!   a full 1080p59.94 preview in a tab is not the goal.
//! - **Lower GPU limits than native.** Write shaders to the WebGPU downlevel
//!   tier and validate against those limits in CI, not against a dev machine's.
//! - **Fonts are loaded as bytes from the CAS and handed to `pmg-text`.** Never
//!   CSS fonts for canvas text — that is the parity killer.
//!
//! Spike S6 measures all of this on the actual booth machines.
//!
//! # M0 status
//!
//! Stub. `wasm-bindgen` arrives with spike S6.

pub fn selftest() -> Result<(), String> {
    Ok(())
}
