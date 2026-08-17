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

use crate::scene::{GlyphSource, RunGlyph};
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

impl GlyphSource for TextEngineGlyphs<'_> {
    fn run(
        &mut self,
        text: &str,
        font: &FontSpec,
        max_width: Option<f32>,
        max_lines: usize,
        align: Align,
    ) -> Option<(Vec<RunGlyph>, f32)> {
        let face = self.engine.db.select(&font.family, font.weight)?;
        // The SAME call the solver measured with, so the wrap drawn is the wrap
        // the box was sized for.
        let lines = self.engine.shape_lines(
            face,
            text,
            font.size,
            &font.features,
            font.tracking,
            Wrap { max_width, max_lines },
        )?;
        let first = lines.first()?;
        let ascent = first.metrics.ascent;
        let line_h = first.metrics.natural_line_height() * font.line_height;
        let widest = lines.iter().fold(0.0f32, |a, l| a.max(l.width));

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
            // Relative to the WIDEST line rather than to `max_width`, because a
            // content-sized text node is exactly as wide as its widest line and
            // the parent box has already centred that node. Using max_width
            // here would centre twice.
            let align_dx = match align {
                Align::Center => ((widest - shaped.width) * 0.5).round(),
                Align::End => (widest - shaped.width).round(),
                _ => 0.0,
            };
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
        Some((out, ascent))
    }
}
