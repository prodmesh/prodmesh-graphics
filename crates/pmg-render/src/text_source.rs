//! The one bridge from `pmg-text`'s glyphs to the renderer's `GlyphSource`.
//!
//! # Why this lives here rather than in each caller
//!
//! It was written twice — once in `pmgctl take` and once in the node — before
//! the browser needed a third copy. Every copy contains the same three lines
//! that decide whether animated text shudders:
//!
//! ```text
//! let (whole, phase) = pmg_text::quantize(g.x);
//! let bmp = engine.glyph(face, g.id, font.size, phase);
//! out.push(RunGlyph { x: whole as f32, .. });
//! ```
//!
//! Rounding `g.x` instead of splitting it is precisely the mistake that makes
//! sliding text shudder (risk R3), and it is the kind of mistake that gets
//! reintroduced in a copy rather than in the original. One copy, used by the
//! CLI, the node, and the WASM designer — which is also what makes the
//! designer's preview trustworthy, since it is running the same code the node
//! runs (ADR 0001).
//!
//! The dependency points renderer → text, not the reverse: the renderer needs
//! glyphs, text does not need a renderer. `GlyphSource` still exists as a trait
//! so a caller can substitute something else.

use crate::scene::{GlyphSource, Run, RunGlyph};
use pmg_scene::doc::{Align, FontSpec};
use pmg_text::{TextEngine, Wrap};

/// Shapes and rasterizes with a [`TextEngine`], at sub-pixel phases.
pub struct TextEngineGlyphs<'a> {
    pub engine: &'a mut TextEngine,
}

impl<'a> TextEngineGlyphs<'a> {
    pub fn new(engine: &'a mut TextEngine) -> Self {
        Self { engine }
    }
}

/// How far right to start a line of `line_w` in a box of `box_w`.
///
/// Whole pixels: text slides horizontally, and a fractional offset applied to
/// every glyph in a line would be a second source of sub-pixel phase on top of
/// the one that exists on purpose (R3).
pub(crate) fn align_offset(align: Align, box_w: f32, line_w: f32) -> f32 {
    match align {
        Align::Center => ((box_w - line_w) * 0.5).round(),
        Align::End => (box_w - line_w).round(),
        // Start, and Stretch — which is what an absent field means and reads as
        // the start edge, because there is nothing to stretch about a line.
        _ => 0.0,
    }
}

impl GlyphSource for TextEngineGlyphs<'_> {
    fn run(
        &mut self,
        text: &str,
        font: &FontSpec,
        max_width: Option<f32>,
        max_lines: usize,
        align: Align,
    ) -> Option<Run> {
        let face = self.engine.db.select(&font.family, font.weight)?;
        // The SAME call the solver measured with, so the wrap drawn is the wrap
        // the box was sized for.
        let lines = self.engine.shape_lines(
            face,
            text,
            font.size,
            &font.features,
            font.tracking,
            Wrap {
                max_width,
                max_lines,
            },
        )?;
        let first = lines.first()?;
        let ascent = first.metrics.ascent;
        let line_h = first.metrics.natural_line_height() * font.line_height;
        let widest = lines.iter().fold(0.0f32, |a, l| a.max(l.width));
        // The whole block, for vertical alignment. The last line contributes a
        // full line height rather than just its ink: a block whose height
        // depended on whether the last line had a descender would jump when the
        // words changed.
        let block_h = lines.len() as f32 * line_h;

        let mut out = Vec::new();
        for (n, shaped) in lines.iter().enumerate() {
            // Rounded, because a baseline at a fractional y would rasterize
            // each line at a different vertical phase and they would not look
            // like one block of type.
            let baseline = (n as f32 * line_h).round();
            // Each line is aligned independently, which single-line text never
            // needed: a centred two-line lyric has two different line widths
            // and centring only the block would leave the shorter line
            // left-aligned inside it.
            //
            // Relative to the node's own BOX, which is what an operator means
            // by centred. This used to align to the widest LINE, on the theory
            // that a text node is exactly as wide as its content and the parent
            // has already centred it — true for a content-sized node, where the
            // box and the widest line are the same number, and simply wrong for
            // the ordinary case of a fixed-width line of type. Centre did
            // nothing at all there, which is exactly what it looked like.
            let align_dx =
                align_offset(align, max_width.unwrap_or(widest).max(widest), shaped.width);
            for g in &shaped.glyphs {
                // Split the fractional position into a whole pixel and a phase, and
                // rasterize AT that phase. Rounding here instead is precisely what
                // makes animated text shudder (R3).
                let (whole, phase) = pmg_text::quantize(g.x);
                let Some(bmp) = self.engine.glyph(face, g.id, font.size, phase) else {
                    continue;
                };
                if bmp.is_empty() {
                    continue;
                }
                out.push(RunGlyph {
                    x: whole as f32 + align_dx,
                    y: baseline,
                    left: bmp.left,
                    top: bmp.top,
                    width: bmp.width,
                    height: bmp.height,
                    coverage: bmp.coverage.clone(),
                });
            }
        }
        Some(Run {
            glyphs: out,
            ascent,
            block_h,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bug: centring did nothing at all on the ordinary case of a
    /// fixed-width line of type. It aligned to the widest LINE, which for a
    /// single line is the line itself — an offset of zero, every time.
    #[test]
    fn centring_is_relative_to_the_box_not_to_the_line() {
        // One line, 100 wide, in a 400 box: 150 of space each side.
        assert_eq!(align_offset(Align::Center, 400.0, 100.0), 150.0);
        assert_eq!(align_offset(Align::End, 400.0, 100.0), 300.0);
        assert_eq!(align_offset(Align::Start, 400.0, 100.0), 0.0);
    }

    /// A content-sized node is exactly as wide as its widest line, so the box
    /// and the line are the same number and alignment has nothing to do. That
    /// was true before and must stay true: it is why the old code looked right.
    #[test]
    fn a_content_sized_node_has_nothing_to_align_within() {
        assert_eq!(align_offset(Align::Center, 100.0, 100.0), 0.0);
        assert_eq!(align_offset(Align::End, 100.0, 100.0), 0.0);
    }

    /// Two lines of different lengths in one box are each aligned on their own,
    /// which is what makes a centred lyric look centred rather than a centred
    /// block of left-aligned lines.
    #[test]
    fn each_line_is_aligned_on_its_own() {
        assert_eq!(align_offset(Align::Center, 400.0, 200.0), 100.0);
        assert_eq!(align_offset(Align::Center, 400.0, 300.0), 50.0);
    }

    /// Whole pixels only. A fractional offset would put a second sub-pixel
    /// phase on top of the deliberate one and soften every line differently.
    #[test]
    fn the_offset_lands_on_a_whole_pixel() {
        let dx = align_offset(Align::Center, 401.0, 100.0);
        assert_eq!(dx, dx.round());
    }

    /// Stretch is what an absent field deserialises to. It must read as the
    /// start edge rather than as a fourth behaviour nobody asked for.
    #[test]
    fn stretch_reads_as_start() {
        assert_eq!(align_offset(Align::Stretch, 400.0, 100.0), 0.0);
    }
}
