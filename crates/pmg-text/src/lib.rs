//! Font loading, shaping, measurement, and glyph rasterization.
//!
//! Spec: `docs/specs/scene-format.md` §3.3 and §7. Risk: [R3](../../docs/RISKS.md),
//! which names this the thing most likely to make the output look amateur.
//!
//! In the **pure set**: no I/O, no platform, no system font database. Fonts
//! arrive as **bytes** and nothing else. That is what makes WYSIWYG survive
//! across machines — a template that named a system font would render
//! differently on the designer's laptop and the booth Mac, silently.
//!
//! # Why glyphs are identical between native and WASM by construction
//!
//! Everything here is **CPU rasterization**. Given the same font bytes, size,
//! glyph and subpixel phase, the coverage bitmap is the same on both targets
//! because it is the same code doing the same arithmetic. The GPU's only
//! contribution is *sampling* that bitmap — which is exactly what spike S1's
//! alpha-atlas figure measured, and why S1 did not need a text stack to be
//! meaningful.
//!
//! # The three hazards from R3, and what is done about each
//!
//! 1. **Subpixel positioning.** Broadcast CG slides text horizontally, and
//!    integer-snapped glyphs visibly shudder. Glyphs are rasterized at
//!    [`SUBPIXEL_PHASES`] fractional offsets and cached per phase.
//! 2. **Antialiasing over a key.** The atlas stores **coverage only** — a single
//!    alpha channel, never premultiplied colour. Premultiplication happens once,
//!    at composite, in the renderer's linear working space (ADR 0012).
//!    Grayscale only; subpixel-RGB AA is meaningless over an alpha key.
//! 3. **Vertical metrics.** See [`Metrics`]. Chosen once, documented, and never
//!    to be changed.

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::sync::Arc;

pub mod atlas;

pub use atlas::{GlyphAtlas, GlyphBitmap, GlyphKey};

/// Fractional horizontal positions a glyph is rasterized at: **⅛ px**.
///
/// Fewer and animated text shudders as glyphs snap between whole pixels; more
/// and the atlas churns for no visible gain. Spike S2 is what confirms ⅛ is the
/// right number on real hardware — until then this is a reasoned default, and
/// it is the single most important constant in this crate.
pub const SUBPIXEL_PHASES: u32 = 8;

/// Quantise a horizontal position into a whole pixel plus a subpixel phase.
///
/// Returns `(whole_px, phase)` where `phase` is in `0..SUBPIXEL_PHASES`.
pub fn quantize(x: f32) -> (i32, u8) {
    let scaled = (x * SUBPIXEL_PHASES as f32).round() as i64;
    let whole = scaled.div_euclid(SUBPIXEL_PHASES as i64) as i32;
    let phase = scaled.rem_euclid(SUBPIXEL_PHASES as i64) as u8;
    (whole, phase)
}

#[derive(Debug, thiserror::Error)]
pub enum TextError {
    #[error("font bytes could not be parsed")]
    BadFont,
    #[error("no face matches family {family:?} weight {weight}")]
    NoFace { family: String, weight: u16 },
}

/// A loaded face. Owns its bytes so a `swash::FontRef` can be made on demand —
/// `FontRef` borrows, and holding one across the cache would fight the borrow
/// checker for no benefit.
struct Face {
    bytes: Arc<Vec<u8>>,
    offset: u32,
    family: String,
    weight: u16,
}

impl Face {
    fn font(&self) -> swash::FontRef<'_> {
        // Offset was validated when the face was added.
        swash::FontRef::from_index(&self.bytes, self.offset as usize)
            .unwrap_or_else(|| swash::FontRef::from_index(&self.bytes, 0).expect("validated"))
    }
}

/// Faces available to a document, loaded from bytes.
///
/// **There is deliberately no system font enumeration here.** A scene may name
/// a system font only through an explicit fallback chain, and Design flags it
/// as *this will differ between machines* (`docs/specs/scene-format.md` §7).
#[derive(Default)]
pub struct FontDb {
    faces: Vec<Face>,
}

/// Index into a [`FontDb`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FaceId(pub u32);

impl FontDb {
    pub fn new() -> Self {
        Self::default()
    }

    /// Load a face from font bytes.
    pub fn add_face(
        &mut self,
        bytes: Vec<u8>,
        family: impl Into<String>,
        weight: u16,
    ) -> Result<FaceId, TextError> {
        let bytes = Arc::new(bytes);
        let font = swash::FontRef::from_index(&bytes, 0).ok_or(TextError::BadFont)?;
        let offset = font.offset;
        self.faces.push(Face {
            bytes,
            offset,
            family: family.into(),
            weight,
        });
        Ok(FaceId(self.faces.len() as u32 - 1))
    }

    /// Pick the closest face for a family and weight.
    ///
    /// Nearest weight within the family, because a template asking for 600 on a
    /// machine that only has 400 and 700 should render rather than fail — a
    /// missing lower third is worse than a slightly wrong one.
    pub fn select(&self, family: &str, weight: u16) -> Option<FaceId> {
        let mut best: Option<(u32, FaceId)> = None;
        for (i, f) in self.faces.iter().enumerate() {
            if !f.family.eq_ignore_ascii_case(family) {
                continue;
            }
            let d = f.weight.abs_diff(weight) as u32;
            if best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, FaceId(i as u32)));
            }
        }
        best.map(|(_, id)| id)
    }

    pub fn len(&self) -> usize {
        self.faces.len()
    }
    pub fn is_empty(&self) -> bool {
        self.faces.is_empty()
    }

    fn face(&self, id: FaceId) -> Option<&Face> {
        self.faces.get(id.0 as usize)
    }
}

/// Vertical metrics for a face at a size, in pixels.
///
/// # The choice that can never be changed
///
/// **OS/2 typographic metrics when the font sets `USE_TYPO_METRICS`
/// (fsSelection bit 7), otherwise hhea.** This is swash's rule, it is the rule
/// browsers use, and adopting it means our text sits where a designer expects
/// rather than two pixels off from every other renderer they have used.
///
/// It reads as conditional, but it cannot drift: **fonts are content-addressed
/// assets**, so for a given font hash the flag — and therefore the metrics — is
/// fixed forever. A template pinned to a font hash gets the same baseline in
/// three years.
///
/// Changing this rule would move every existing template by a couple of pixels,
/// silently. Do not.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Metrics {
    pub ascent: f32,
    pub descent: f32,
    pub leading: f32,
    pub units_per_em: u16,
}

impl Metrics {
    /// Distance from one baseline to the next, before the document's
    /// `line_height` multiplier is applied.
    pub fn natural_line_height(&self) -> f32 {
        self.ascent + self.descent + self.leading
    }
}

/// One glyph, positioned on the baseline.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PositionedGlyph {
    pub id: u16,
    /// Horizontal offset from the run origin, in pixels. Fractional on purpose.
    pub x: f32,
    /// Vertical offset from the baseline, in pixels.
    pub y: f32,
}

/// A shaped run of text.
#[derive(Debug, Clone, PartialEq)]
pub struct ShapedRun {
    pub glyphs: Vec<PositionedGlyph>,
    pub width: f32,
    pub metrics: Metrics,
}

/// Shaping, measurement, and rasterization for one document's fonts.
pub struct TextEngine {
    pub db: FontDb,
    atlas: GlyphAtlas,
    /// Cached metrics per (face, quantised size).
    metrics: HashMap<(FaceId, u32), Metrics>,
}

impl TextEngine {
    pub fn new(db: FontDb) -> Self {
        Self {
            db,
            atlas: GlyphAtlas::new(),
            metrics: HashMap::new(),
        }
    }

    pub fn atlas(&self) -> &GlyphAtlas {
        &self.atlas
    }

    /// Vertical metrics for a face at a size. See [`Metrics`] for the rule.
    pub fn metrics(&mut self, face: FaceId, size_px: f32) -> Option<Metrics> {
        let key = (face, size_px.to_bits());
        if let Some(m) = self.metrics.get(&key) {
            return Some(*m);
        }
        let f = self.db.face(face)?;
        let font = f.font();
        // scale(ppem), NOT linear_scale(). linear_scale multiplies raw design
        // units by its argument; scale() divides by units_per_em first, which is
        // what turns design units into pixels. Getting this wrong gives an
        // ascent of 85284 "px" at 46 px and is invisible to any test that only
        // checks relationships — both numbers are wrong by the same factor.
        let m = font.metrics(&[]).scale(size_px);
        let out = Metrics {
            ascent: m.ascent,
            descent: m.descent,
            leading: m.leading,
            units_per_em: m.units_per_em,
        };
        self.metrics.insert(key, out);
        Some(out)
    }

    /// Shape a run.
    ///
    /// Features are passed through from the document. **`tnum` matters** —
    /// clocks and scores jitter without tabular figures
    /// (`docs/specs/scene-format.md` §3.3).
    pub fn shape(
        &mut self,
        face: FaceId,
        text: &str,
        size_px: f32,
        features: &[String],
        tracking: f32,
    ) -> Option<ShapedRun> {
        let metrics = self.metrics(face, size_px)?;
        let f = self.db.face(face)?;
        let font = f.font();

        // (&str, u16) converts straight into swash's Setting. Value 1 means
        // "on"; a future format revision that wants alternates would carry the
        // value rather than just the tag.
        let feats: Vec<swash::Setting<u16>> =
            features.iter().map(|t| (t.as_str(), 1u16).into()).collect();

        let mut ctx = swash::shape::ShapeContext::new();
        let mut shaper = ctx
            .builder(font)
            .size(size_px)
            .features(feats.iter().copied())
            .build();
        shaper.add_str(text);

        let mut glyphs = Vec::new();
        let mut pen = 0.0f32;
        shaper.shape_with(|cluster| {
            for g in cluster.glyphs {
                glyphs.push(PositionedGlyph {
                    id: g.id,
                    x: pen + g.x,
                    y: g.y,
                });
                // Tracking is a fraction of em, applied per glyph advance.
                pen += g.advance + tracking * size_px;
            }
        });

        Some(ShapedRun {
            glyphs,
            width: pen.max(0.0),
            metrics,
        })
    }

    /// Rasterize a glyph at a subpixel phase, caching the result.
    ///
    /// Returns **alpha coverage**, never premultiplied colour — see the module
    /// note on hazard 2.
    pub fn glyph(
        &mut self,
        face: FaceId,
        glyph: u16,
        size_px: f32,
        phase: u8,
    ) -> Option<&GlyphBitmap> {
        let key = GlyphKey {
            face,
            size_bits: size_px.to_bits(),
            glyph,
            phase,
        };
        if !self.atlas.contains(&key) {
            let f = self.db.face(face)?;
            let font = f.font();
            let bmp = atlas::rasterize(font, glyph, size_px, phase)?;
            self.atlas.insert(key, bmp);
        }
        self.atlas.get(&key)
    }
}

/// Real text measurement for the layout solver.
///
/// Replaces `pmg_layout::StubMeasure`. M1b's tests assert *relationships*
/// between rectangles rather than absolute widths precisely so they keep
/// passing when this takes over.
///
/// Wraps the engine in a `RefCell` because measuring populates caches —
/// `TextMeasure::measure` takes `&self` since the solver calls it from a
/// callback, and shaping legitimately mutates.
pub struct Measurer(std::cell::RefCell<TextEngine>);

impl Measurer {
    pub fn new(engine: TextEngine) -> Self {
        Self(std::cell::RefCell::new(engine))
    }
    /// Take the engine back, with its caches warm — the renderer wants the
    /// atlas that measuring just filled.
    pub fn into_engine(self) -> TextEngine {
        self.0.into_inner()
    }
    pub fn borrow_mut(&self) -> std::cell::RefMut<'_, TextEngine> {
        self.0.borrow_mut()
    }
}

impl pmg_layout::TextMeasure for Measurer {
    fn measure(
        &self,
        text: &str,
        font: &pmg_scene::doc::FontSpec,
        max_width: Option<f32>,
    ) -> (f32, f32) {
        let mut eng = self.0.borrow_mut();
        let Some(face) = eng.db.select(&font.family, font.weight) else {
            // A missing face measures as nothing rather than panicking. The
            // document is still laid out, and the absent font is a validation
            // concern rather than a render-thread failure.
            return (0.0, 0.0);
        };
        let Some(run) = eng.shape(face, text, font.size, &font.features, font.tracking) else {
            return (0.0, 0.0);
        };
        let line_h = run.metrics.natural_line_height() * font.line_height;
        match max_width {
            Some(mw) if mw > 0.0 && run.width > mw => {
                // M1c does not wrap: a single line, clipped by the box. Real
                // line breaking arrives with parley in M5, and until then a
                // template that overflows is a design-time problem the editor
                // can show rather than a silent runtime reflow.
                (mw, line_h)
            }
            _ => (run.width, line_h),
        }
    }
}

pub fn selftest() -> Result<(), String> {
    // Pure arithmetic first — these hold with or without a font on the machine.
    for (x, want_px, want_phase) in [
        (0.0, 0, 0u8),
        (1.0, 1, 0),
        (1.125, 1, 1),
        (1.5, 1, 4),
        (1.875, 1, 7),
        (2.0, 2, 0),
    ] {
        let (px, ph) = quantize(x);
        if px != want_px || ph != want_phase {
            return Err(format!(
                "quantize({x}) = ({px},{ph}), expected ({want_px},{want_phase})"
            ));
        }
    }
    // Negative positions must quantise consistently, not toward zero — text
    // sliding in from the left crosses zero and must not jump there.
    let (px, ph) = quantize(-0.125);
    if px != -1 || ph != 7 {
        return Err(format!("quantize(-0.125) = ({px},{ph}), expected (-1,7)"));
    }
    if SUBPIXEL_PHASES != 8 {
        return Err("SUBPIXEL_PHASES changed — every cached glyph phase depends on it".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selftest_passes() {
        selftest().unwrap();
    }

    #[test]
    fn quantize_round_trips_within_an_eighth() {
        for i in 0..200 {
            let x = i as f32 * 0.05 - 5.0;
            let (px, ph) = quantize(x);
            let back = px as f32 + ph as f32 / SUBPIXEL_PHASES as f32;
            assert!(
                (back - x).abs() <= 0.5 / SUBPIXEL_PHASES as f32 + 1e-6,
                "quantize({x}) -> {back} is further than half a phase away"
            );
        }
    }
}
