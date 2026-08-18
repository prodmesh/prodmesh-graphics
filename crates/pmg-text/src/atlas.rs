//! Glyph rasterization and the coverage atlas.
//!
//! # Coverage, never colour
//!
//! Every bitmap here is a **single alpha channel**: how much of the pixel the
//! glyph covers, and nothing else. It is never premultiplied, never coloured,
//! and never subpixel-RGB.
//!
//! That matters for two separate reasons:
//!
//! - **Subpixel-RGB antialiasing is meaningless over an alpha key.** There is no
//!   known background to tune the fringes against, and the result is coloured
//!   edges over live video.
//! - **Premultiplication belongs at composite, once**, in the renderer's linear
//!   working space. Baking colour into the atlas would premultiply in the wrong
//!   space and give the dark-halo artefact ADR 0012 exists to prevent — and it
//!   would make the cache per-colour, which is absurd.
//!
//! # Hinting is off, deliberately
//!
//! Hinting grid-fits outlines to whole pixels. That is right for small static
//! UI text and wrong here: a hinted glyph *snaps* as it moves, which is exactly
//! the shudder [`crate::SUBPIXEL_PHASES`] exists to avoid. Broadcast CG slides
//! text, so smooth beats crisp.

use crate::FaceId;
use std::collections::HashMap;
use swash::scale::{Render, ScaleContext, Source, StrikeWith};
use swash::zeno::{Format, Vector};

/// Identity of one rasterized glyph.
///
/// The **phase** is part of the key. Two draws of the same glyph at different
/// subpixel offsets are different bitmaps, and conflating them is the bug that
/// makes animated text shudder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GlyphKey {
    pub face: FaceId,
    /// `f32::to_bits` of the pixel size, so the key is exact rather than
    /// rounded — two sizes that differ by a hair are genuinely different
    /// rasterizations.
    pub size_bits: u32,
    pub glyph: u16,
    pub phase: u8,
}

/// A rasterized glyph: alpha coverage plus its placement relative to the pen.
#[derive(Debug, Clone, PartialEq)]
pub struct GlyphBitmap {
    pub width: u32,
    pub height: u32,
    /// Offset from the pen position to the bitmap's left edge, in pixels.
    pub left: i32,
    /// Offset from the baseline to the bitmap's top edge, in pixels.
    /// Positive is up.
    pub top: i32,
    /// `width * height` bytes of **coverage**. Not premultiplied. Not colour.
    pub coverage: Vec<u8>,
}

impl GlyphBitmap {
    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }

    /// Coverage at a pixel, or 0 outside.
    pub fn at(&self, x: u32, y: u32) -> u8 {
        if x >= self.width || y >= self.height {
            return 0;
        }
        self.coverage[(y * self.width + x) as usize]
    }
}

/// A cache of rasterized glyphs, keyed by face, size, glyph, and subpixel phase.
///
/// Deliberately unbounded for now. A full-screen ticker is the case that would
/// need eviction, and spike S2 measures whether it actually does — guessing at
/// an eviction policy before there is a measurement would be inventing a
/// problem.
#[derive(Default)]
pub struct GlyphAtlas {
    glyphs: HashMap<GlyphKey, GlyphBitmap>,
}

impl GlyphAtlas {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn contains(&self, key: &GlyphKey) -> bool {
        self.glyphs.contains_key(key)
    }
    pub fn get(&self, key: &GlyphKey) -> Option<&GlyphBitmap> {
        self.glyphs.get(key)
    }
    pub fn insert(&mut self, key: GlyphKey, bmp: GlyphBitmap) {
        self.glyphs.insert(key, bmp);
    }
    pub fn len(&self) -> usize {
        self.glyphs.len()
    }
    pub fn is_empty(&self) -> bool {
        self.glyphs.is_empty()
    }
    /// Total coverage bytes held. Reported by telemetry so a ticker that is
    /// churning the cache is visible rather than merely slow.
    pub fn bytes(&self) -> usize {
        self.glyphs.values().map(|g| g.coverage.len()).sum()
    }
}

/// Rasterize one glyph at a subpixel phase.
///
/// CPU only. Given the same font bytes, size, glyph and phase this produces the
/// same bytes on every platform and both build targets — which is what makes
/// glyphs identical between the node and the browser *by construction* rather
/// than by measurement.
/// Coverage gamma — "stem darkening", and the reason it is needed here.
///
/// Geometric coverage is the fraction of a pixel the glyph covers. Composited
/// in **linear light** that fraction is exactly right, and this table would be
/// an error. But the fill and key leaving this system are composited by
/// whatever is downstream — a switcher's keyer, a wall processor — and those
/// blend the encoded values directly. `out = a*fg + (1-a)*bg` performed on
/// gamma-encoded numbers puts a 50%-covered edge of white type at 128/255,
/// which *displays* as 21% of the light it should carry. Every antialiased
/// edge comes out too dark, so type reads thin and the edges look crunchy.
///
/// We cannot fix the switcher, so the compensation goes where the coverage is
/// made. This is what FreeType calls stem darkening and what Skia does with its
/// contrast/gamma tables; it is standard practice rather than a fudge.
///
/// **It is an approximation, and here is exactly how.** The correction that
/// would be exact depends on the foreground and background luminance of each
/// pixel, and the background is live camera this process never sees. One
/// exponent is the best a rasterizer can do without knowing what it will be
/// keyed over. It is tuned for the case that matters and is worst: light type
/// over a dark stage.
///
/// Applied to the atlas, so it lands on the KEY as well as the fill — which is
/// the point. The key is what the switcher blends with.
const COVERAGE_GAMMA: f32 = 1.0 / 1.45;

/// Bend one coverage byte.
///
/// 0 and 255 are fixed points, so a fully covered pixel stays opaque and an
/// empty one stays empty: this changes only the edge.
fn darken(c: u8) -> u8 {
    // A 256-entry table would be faster and is not worth the static: this runs
    // once per glyph per subpixel phase, then the result is cached in the atlas
    // for the life of the process.
    let a = c as f32 / 255.0;
    (a.powf(COVERAGE_GAMMA) * 255.0 + 0.5).clamp(0.0, 255.0) as u8
}

pub fn rasterize(
    font: swash::FontRef<'_>,
    glyph: u16,
    size_px: f32,
    phase: u8,
) -> Option<GlyphBitmap> {
    let offset_x = phase as f32 / crate::SUBPIXEL_PHASES as f32;

    let mut ctx = ScaleContext::new();
    let mut scaler = ctx
        .builder(font)
        .size(size_px)
        // See the module note: hinting snaps glyphs to the pixel grid, which is
        // the shudder we are avoiding.
        .hint(false)
        .build();

    let image = Render::new(&[
        // Outlines first. The bitmap and colour sources are fallbacks for fonts
        // that only ship those; COLRv1 emoji arrive via ColorOutline.
        Source::ColorOutline(0),
        Source::ColorBitmap(StrikeWith::BestFit),
        Source::Outline,
    ])
    // Alpha coverage — a single channel. Never subpixel-RGB (meaningless over
    // an alpha key) and never premultiplied colour.
    .format(Format::Alpha)
    .offset(Vector::new(offset_x, 0.0))
    .render(&mut scaler, glyph)?;

    Some(GlyphBitmap {
        width: image.placement.width,
        height: image.placement.height,
        left: image.placement.left,
        top: image.placement.top,
        // Pure arithmetic on the CPU, so the node and the browser get the same
        // bytes by construction — the same property the rest of this function
        // is written to hold.
        coverage: image.data.iter().copied().map(darken).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_bitmap_reads_as_zero_everywhere() {
        let b = GlyphBitmap {
            width: 0,
            height: 0,
            left: 0,
            top: 0,
            coverage: vec![],
        };
        assert!(b.is_empty());
        assert_eq!(b.at(0, 0), 0);
        assert_eq!(b.at(99, 99), 0);
    }

    #[test]
    fn phase_participates_in_the_cache_key() {
        // If phase were dropped from the key, every glyph would render at one
        // offset and animated text would snap between whole pixels — the exact
        // failure R3 names first.
        let a = GlyphKey {
            face: FaceId(0),
            size_bits: 46f32.to_bits(),
            glyph: 42,
            phase: 0,
        };
        let b = GlyphKey { phase: 3, ..a };
        assert_ne!(a, b);

        let mut atlas = GlyphAtlas::new();
        atlas.insert(
            a,
            GlyphBitmap {
                width: 1,
                height: 1,
                left: 0,
                top: 0,
                coverage: vec![255],
            },
        );
        assert!(atlas.contains(&a));
        assert!(!atlas.contains(&b), "different phases must not collide");
    }
}
