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

pub mod corpus;
pub mod draw;
pub mod glyph_fx;
pub mod interact;
pub mod parity;
pub mod scene;
pub mod text_source;
pub mod timeline;
pub mod tween;
pub mod ycbcr;

pub use draw::SceneRenderer;
pub use scene::{
    CoverageAtlas, FrameNode, GlyphSource, PlacedGlyph, RenderContent, RenderNode, RunGlyph,
    SceneInstance,
};
pub use tween::{ease, tween_frames, TweenFrames};

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

/// Where the scene is composited, before it is encoded to [`TARGET_FORMAT`].
///
/// **Linear light, premultiplied, 16-bit float** — ADR 0012's "composite in
/// linear, encode once at output", which the renderer described and did not do
/// until 2026-08-17.
///
/// Blending coverage into gamma-encoded values makes every antialiased edge too
/// dark: a 50%-covered edge of white text stored 128/255, which is 21% of the
/// light it should carry, so text read thin and crunchy against a dark key.
/// Worst at maximum contrast — white type with no plate over camera — and
/// nearly invisible on the cream-on-teal scripture block, which is exactly how
/// it survived this long.
///
/// 16-bit float rather than 8-bit linear: 8 bits of linear light bands visibly
/// in the darks, which is the whole reason the sRGB curve exists.
pub const COMPOSITE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// sRGB electro-optical transfer function, on one channel.
///
/// The exact piecewise curve, not the 2.2 approximation: the linear toe matters
/// for precisely the near-black values an antialiased edge is made of, and
/// getting it approximately right here would leave a smaller version of the
/// bug this replaced.
pub fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.040_45 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Straight (un-premultiplied) sRGB colour to straight linear.
///
/// **Alpha is not converted.** Alpha is coverage — a geometric fraction, already
/// linear — and putting it through a transfer function would be a second bug
/// wearing the first one's clothes.
pub fn color_to_linear(c: [f32; 4]) -> [f32; 4] {
    [
        srgb_to_linear(c[0]),
        srgb_to_linear(c[1]),
        srgb_to_linear(c[2]),
        c[3],
    ]
}

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
