//! The wgpu render graph: `SceneInstance` in, texture out.
//!
//! Decision: ADR 0002. This crate is the reason ADR 0001 works.
//!
//! # The invariant
//!
//! **No I/O. No platform.** This crate takes a `SceneInstance`, a
//! `wgpu::Device`, and a target texture. Nothing else. That signature is what
//! lets it compile to `wasm32-unknown-unknown` unchanged and run in the Design
//! canvas, and it is enforced by a required CI job rather than by review —
//! a lint would drift, a failing build cannot (ADR 0004).
//!
//! If you find yourself wanting a file handle, a `tokio::spawn`, or a vendor
//! SDK in here, the thing you want belongs in `pmg-node` or `pmg-wasm`.
//!
//! # Rules that keep native and browser output the same
//!
//! - **Never enable MSAA.** Sample patterns are unspecified across backends,
//!   which makes it a parity hazard. All antialiasing is done in-shader,
//!   analytically or with deterministic ordered supersampling.
//! - Never rely on sampler filtering defaults.
//! - One pinned wgpu version across both targets. **A wgpu bump is a
//!   release-gated change requiring a full golden-image re-run** (risk R11).
//!
//! Even with all of that, native and browser output will not be bit-identical —
//! see risk R1. What is guaranteed is identical layout, glyphs, bindings, and
//! timing. Spike S1 measures the rest and bounds it.
//!
//! # M0 status
//!
//! Types only. `wgpu` is deliberately not a dependency yet: adding it before
//! spike S1 has run would be committing to a version before we know the
//! approach holds.

#![forbid(unsafe_code)]

/// Set on a rendered instance when the document contained something this
/// reader did not understand.
///
/// An unknown node kind draws a magenta placeholder and sets this. Rendering
/// nothing and looking fine is the dangerous behaviour
/// (`docs/specs/scene-format.md` §3.4).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Degraded(pub bool);

pub fn selftest() -> Result<(), String> {
    Ok(())
}
