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
use pmg_scene::doc::FontSpec;
use pmg_text::TextEngine;

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
    fn run(&mut self, text: &str, font: &FontSpec) -> Option<(Vec<RunGlyph>, f32)> {
        let face = self.engine.db.select(&font.family, font.weight)?;
        let shaped = self
            .engine
            .shape(face, text, font.size, &font.features, font.tracking)?;
        let ascent = shaped.metrics.ascent;

        let mut out = Vec::new();
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
                x: whole as f32,
                left: bmp.left,
                top: bmp.top,
                width: bmp.width,
                height: bmp.height,
                coverage: bmp.coverage.clone(),
            });
        }
        Some((out, ascent))
    }
}
