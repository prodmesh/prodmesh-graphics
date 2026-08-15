//! Flexbox (Yoga model) layout solver over a `TextMeasure` trait.
//!
//! Spec: `docs/specs/scene-format.md` §4. Decision: ADR 0005.
//!
//! In the **pure set**: no I/O, no platform. Given a tree and a text measurer,
//! it returns resolved rectangles — same input, same output, on every platform
//! and both targets. This is the half of ADR 0001's promise that *is*
//! guaranteed, and spike S1 checks it by comparing numbers before it compares
//! pixels.
//!
//! # The rule this crate exists to make cheap
//!
//! `layout.*` affects siblings and forces a relayout. `transform.*` does not.
//! A responsive lower third sizes itself **once**, when data binds
//! (`width: Content` in a row), and reveals with a transform and a clip — which
//! costs nothing per frame. `pmg-anim` targeting a `layout.*` property is a
//! warning the compiler records in the manifest.
//!
//! # M0 status
//!
//! Types only. The solver is M1.

#![forbid(unsafe_code)]

/// A resolved rectangle in design space.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// How a box is sized along an axis.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Dimension {
    Px(f32),
    Percent(f32),
    Auto,
    /// Size to the measured content. On a text node this calls [`TextMeasure`],
    /// and it is what makes the shrink-and-grow lower third fall out of the
    /// layout rather than being special-cased.
    Content,
}

/// Text measurement, supplied by `pmg-text`.
///
/// A trait rather than a dependency so that `pmg-layout` stays pure and
/// testable with a stub measurer — and so that swapping the text stack (see
/// spike S2's fallback) does not reach into the layout solver.
pub trait TextMeasure {
    /// Measure a run at an optional width constraint, returning `(width, height)`.
    fn measure(&self, text: &str, max_width: Option<f32>) -> (f32, f32);
}

pub fn selftest() -> Result<(), String> {
    Ok(())
}
