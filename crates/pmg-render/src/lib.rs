//! The wgpu render graph: `SceneInstance` in, texture out.
//!
//! Decision: ADR 0002. This crate is the reason ADR 0001 works.
//!
//! # The invariant
//!
//! **No I/O. No platform.** This crate takes geometry, a `wgpu::Device`, and a
//! target texture. Nothing else. That signature is what lets it compile to
//! `wasm32-unknown-unknown` unchanged and run in the Design canvas, and it is
//! enforced by a required CI job rather than by review (ADR 0004).
//!
//! Note what is *absent*: there is no readback function here. Mapping a buffer
//! is async, and how you wait differs by platform — native polls, wasm awaits.
//! Putting that boundary in the caller is what keeps this crate platform-free.
//!
//! # M0 status
//!
//! The parity corpus for spike S1, and nothing else. Three figures that between
//! them cover what a GPU actually does differently: fill, our own antialiasing
//! math, and texture sampling.

#![forbid(unsafe_code)]

pub mod draw;
pub mod parity;
pub mod scene;
pub mod text_source;
pub mod timeline;

pub use draw::SceneRenderer;
pub use scene::{
    CoverageAtlas, FrameNode, GlyphSource, PlacedGlyph, RenderContent, RenderNode, RunGlyph,
    SceneInstance,
};

/// Set on a rendered instance when the document contained something this
/// reader did not understand.
///
/// An unknown node kind draws a magenta placeholder and sets this. Rendering
/// nothing and looking fine is the dangerous behaviour
/// (`docs/specs/scene-format.md` §3.4).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Degraded(pub bool);

/// The colour format every parity render targets.
///
/// **`Rgba8Unorm`, deliberately not `Rgba8UnormSrgb`.** Automatic sRGB
/// conversion on write is one more thing that could differ between a native
/// backend and a browser's; keeping it out of the pipeline means any encode is
/// ours, in-shader, identical on both sides. See `parity.wgsl`.
pub const TARGET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

pub fn selftest() -> Result<(), String> {
    parity::selftest()
}

/// What a draw does to whatever is already in the target.
///
/// The distinction exists because an output is a **stack of layers**, not one
/// picture: the bottom layer starts from nothing and every layer above it
/// composites over the result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Load {
    /// Start from transparent black.
    Clear,
    /// Keep what is there and composite over it, premultiplied source-over.
    Over,
}
