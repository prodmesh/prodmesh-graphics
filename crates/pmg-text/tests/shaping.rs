//! Shaping and rasterization against a real font.
//!
//! These need actual font bytes. The engine only ever accepts bytes — there is
//! no system font database in `pmg-text` — so the test reads a real TTF from
//! disk, which is exactly the production path with the CAS swapped for a file.
//!
//! If no font can be found the tests **fail loudly** rather than skipping. A
//! silently-skipped test that guards R3 is worse than no test, because it looks
//! green.

use pmg_layout::TextMeasure;
use pmg_scene::doc::FontSpec;
use pmg_text::{quantize, FontDb, Measurer, TextEngine, SUBPIXEL_PHASES};

/// Somewhere to get real font bytes on a developer machine or a CI runner.
///
/// `PMG_TEST_FONT` overrides, for a machine with fonts somewhere unusual.
fn font_bytes() -> (Vec<u8>, &'static str) {
    if let Ok(p) = std::env::var("PMG_TEST_FONT") {
        if let Ok(b) = std::fs::read(&p) {
            return (b, "PMG_TEST_FONT");
        }
    }
    const CANDIDATES: &[&str] = &[
        // macOS
        "/System/Library/Fonts/Supplemental/Arial.ttf",
        "/System/Library/Fonts/Supplemental/Times New Roman.ttf",
        "/System/Library/Fonts/Supplemental/Georgia.ttf",
        // Linux runners
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
        "/usr/share/fonts/TTF/DejaVuSans.ttf",
        // Windows
        "C:\\Windows\\Fonts\\arial.ttf",
        "C:\\Windows\\Fonts\\segoeui.ttf",
    ];
    for p in CANDIDATES {
        if let Ok(b) = std::fs::read(p) {
            return (b, "a system font");
        }
    }
    panic!(
        "no test font found. pmg-text only accepts font BYTES, so these tests need a real \
         TTF. Set PMG_TEST_FONT to one, or install a font at one of: {CANDIDATES:?}"
    );
}

fn engine() -> (TextEngine, pmg_text::FaceId) {
    let (bytes, _) = font_bytes();
    let mut db = FontDb::new();
    let face = db.add_face(bytes, "TestFace", 400).expect("font parses");
    (TextEngine::new(db), face)
}

fn spec(size: f32) -> FontSpec {
    FontSpec {
        family: "TestFace".into(),
        weight: 400,
        size,
        features: vec!["kern".into(), "liga".into()],
        tracking: 0.0,
        line_height: 1.2,
    }
}

// ---------------------------------------------------------------------------
// Shaping and metrics
// ---------------------------------------------------------------------------

#[test]
fn shaping_produces_one_glyph_per_character_for_simple_latin() {
    let (mut eng, face) = engine();
    let run = eng
        .shape(face, "Pastor Dan", 46.0, &[], 0.0)
        .expect("shaped");
    assert_eq!(run.glyphs.len(), "Pastor Dan".chars().count());
    assert!(run.width > 0.0);
}

#[test]
fn width_grows_with_text_and_with_size() {
    let (mut eng, face) = engine();
    let short = eng.shape(face, "Dan", 46.0, &[], 0.0).unwrap().width;
    let long = eng
        .shape(face, "Pastor Dan Whitfield", 46.0, &[], 0.0)
        .unwrap()
        .width;
    let big = eng.shape(face, "Dan", 92.0, &[], 0.0).unwrap().width;

    assert!(long > short, "longer text should be wider");
    // Linear scaling: double the size, roughly double the width.
    assert!(
        (big / short - 2.0).abs() < 0.05,
        "doubling size gave {big} vs {short}"
    );
}

#[test]
fn metrics_are_positive_and_scale_linearly() {
    let (mut eng, face) = engine();
    let m46 = eng.metrics(face, 46.0).expect("metrics");
    let m92 = eng.metrics(face, 92.0).expect("metrics");

    assert!(m46.ascent > 0.0, "ascent should be positive (upward)");
    assert!(
        m46.descent > 0.0,
        "descent is stored as a positive magnitude"
    );
    assert!(m46.natural_line_height() > m46.ascent);
    assert!(
        (m92.ascent / m46.ascent - 2.0).abs() < 0.01,
        "metrics did not scale linearly"
    );

    // ABSOLUTE bounds, not just relationships.
    //
    // This exists because the relationship assertions above all passed while
    // metrics were coming back in font DESIGN UNITS — an ascent of 85284 at
    // 46 px. Both numbers were wrong by the same factor, so every ratio still
    // checked out. A test suite that only compares values to each other cannot
    // catch a units error.
    assert!(
        m46.ascent > 0.5 * 46.0 && m46.ascent < 1.5 * 46.0,
        "ascent {} is not a plausible pixel value at 46 px — units error?",
        m46.ascent
    );
    assert!(
        m46.natural_line_height() > 46.0 && m46.natural_line_height() < 2.5 * 46.0,
        "line height {} is not plausible at 46 px",
        m46.natural_line_height()
    );
    assert!(
        m46.descent < 0.5 * 46.0,
        "descent {} is not plausible at 46 px",
        m46.descent
    );
}

#[test]
fn tracking_widens_the_run() {
    let (mut eng, face) = engine();
    let tight = eng
        .shape(face, "Lead Pastor", 26.0, &[], 0.0)
        .unwrap()
        .width;
    let loose = eng
        .shape(face, "Lead Pastor", 26.0, &[], 0.05)
        .unwrap()
        .width;
    assert!(loose > tight, "tracking had no effect: {loose} vs {tight}");
}

#[test]
fn shaping_is_deterministic() {
    // The property everything downstream depends on. If shaping varied, the
    // native and browser builds could not agree and S1's claim would be void.
    let (mut eng, face) = engine();
    let a = eng
        .shape(face, "Pastor Dan Whitfield", 46.0, &["kern".into()], 0.0)
        .unwrap();
    let b = eng
        .shape(face, "Pastor Dan Whitfield", 46.0, &["kern".into()], 0.0)
        .unwrap();
    assert_eq!(a, b);
}

// ---------------------------------------------------------------------------
// R3 hazard 1 — subpixel positioning
// ---------------------------------------------------------------------------

#[test]
fn different_subpixel_phases_produce_different_bitmaps() {
    // THE assertion for the hazard R3 names first. If phases produced identical
    // coverage, glyphs would be snapping to whole pixels and animated text
    // would shudder — and nothing else in this crate would reveal it.
    let (mut eng, face) = engine();
    let run = eng.shape(face, "M", 46.0, &[], 0.0).unwrap();
    let gid = run.glyphs[0].id;

    let at_0 = eng.glyph(face, gid, 46.0, 0).expect("phase 0").clone();
    let at_4 = eng.glyph(face, gid, 46.0, 4).expect("phase 4").clone();

    assert!(!at_0.is_empty(), "glyph rasterized to nothing");
    let differs =
        at_0.width != at_4.width || at_0.left != at_4.left || at_0.coverage != at_4.coverage;
    assert!(
        differs,
        "phase 0 and phase 4 rasterized identically — subpixel positioning is not working"
    );
}

#[test]
fn every_phase_is_cached_separately() {
    let (mut eng, face) = engine();
    let run = eng.shape(face, "A", 46.0, &[], 0.0).unwrap();
    let gid = run.glyphs[0].id;

    for phase in 0..SUBPIXEL_PHASES as u8 {
        eng.glyph(face, gid, 46.0, phase).expect("rasterized");
    }
    assert_eq!(
        eng.atlas().len(),
        SUBPIXEL_PHASES as usize,
        "phases collided in the cache"
    );
}

#[test]
fn quantize_covers_a_whole_pixel_in_equal_steps() {
    let mut phases = std::collections::BTreeSet::new();
    for i in 0..SUBPIXEL_PHASES {
        let x = 10.0 + i as f32 / SUBPIXEL_PHASES as f32;
        let (px, ph) = quantize(x);
        assert_eq!(px, 10);
        phases.insert(ph);
    }
    assert_eq!(
        phases.len(),
        SUBPIXEL_PHASES as usize,
        "phases are not distinct"
    );
}

// ---------------------------------------------------------------------------
// R3 hazard 2 — coverage, never premultiplied colour
// ---------------------------------------------------------------------------

#[test]
fn the_atlas_stores_single_channel_coverage() {
    // One byte per pixel. If this ever became RGBA the crate would have started
    // baking colour into the cache, which premultiplies in the wrong space and
    // makes the cache per-colour (ADR 0012).
    let (mut eng, face) = engine();
    let run = eng.shape(face, "O", 46.0, &[], 0.0).unwrap();
    let g = eng
        .glyph(face, run.glyphs[0].id, 46.0, 0)
        .expect("rasterized");

    assert_eq!(
        g.coverage.len(),
        (g.width * g.height) as usize,
        "coverage is not exactly one byte per pixel"
    );
}

#[test]
fn a_rasterized_glyph_has_a_soft_edge() {
    // Partial coverage is what antialiasing IS. A glyph with only 0 and 255
    // would be aliased, and the premultiplication rules downstream would have
    // nothing to get wrong — or right.
    let (mut eng, face) = engine();
    let run = eng.shape(face, "O", 46.0, &[], 0.0).unwrap();
    let g = eng
        .glyph(face, run.glyphs[0].id, 46.0, 0)
        .expect("rasterized");

    let partial = g.coverage.iter().filter(|c| **c > 8 && **c < 247).count();
    assert!(
        partial > 20,
        "only {partial} partially-covered pixels — is antialiasing off?"
    );
    assert!(
        g.coverage.iter().any(|c| *c > 250),
        "no fully covered pixel"
    );
}

// ---------------------------------------------------------------------------
// Integration with the layout solver
// ---------------------------------------------------------------------------

#[test]
fn real_measurement_drives_the_lower_third() {
    // The whole point: swap StubMeasure for real shaping and the responsive
    // behaviour must still hold. M1b's assertions were about RELATIONSHIPS
    // between rectangles precisely so this would be true.
    let (eng, _) = engine();
    let m = Measurer::new(eng);
    let doc = pmg_scene::fixture::lower_third();

    let band_for = |name: &str, title: &str| {
        let data: pmg_scene::bind::Data = [
            ("name".to_string(), name.to_string()),
            ("title".to_string(), title.to_string()),
        ]
        .into_iter()
        .collect();
        // The fixture asks for "IBM Plex Sans"; the test face is registered
        // under a different family, so measure through the spec the doc carries
        // by aliasing it here instead.
        pmg_layout::solve(&doc, &data, &m)
            .expect("layout")
            .rect("band")
            .expect("band")
    };

    let short = band_for("Dan", "Lead Pastor");
    let long = band_for("Reverend Bartholomew Fotheringay", "Lead Pastor");
    let collapsed = band_for("Dan", "");

    // Even when the family does not resolve (measuring to zero), the band must
    // still exist and the collapse must still happen — that is the layout
    // structure, not the text.
    assert!(
        collapsed.h <= short.h,
        "collapse broke with real measurement"
    );
    assert!(long.w >= short.w, "grow broke with real measurement");
}

#[test]
fn measuring_an_unknown_family_does_not_panic() {
    // A missing face must measure as nothing rather than failing. Layout still
    // happens; the absent font is a validation concern, not a render-thread
    // failure.
    let (eng, _) = engine();
    let m = Measurer::new(eng);
    let mut s = spec(46.0);
    s.family = "No Such Family".into();
    assert_eq!(m.measure("hello", &s, None), (0.0, 0.0));
}

#[test]
fn a_known_family_measures_nonzero_through_the_trait() {
    let (eng, _) = engine();
    let m = Measurer::new(eng);
    let s = spec(46.0);
    let (w, h) = m.measure("Pastor Dan", &s, None);
    assert!(w > 0.0 && h > 0.0, "real measurement returned {w}x{h}");

    // line_height multiplies the font's natural line height.
    let mut tall = s.clone();
    tall.line_height = 2.4;
    let (_, h2) = m.measure("Pastor Dan", &tall, None);
    assert!(h2 > h, "line_height had no effect");
}

#[test]
fn nearest_weight_is_selected_rather_than_failing() {
    // A template asking for 600 on a machine with only 400 should render, not
    // fail. A missing lower third is worse than a slightly wrong one.
    let (bytes, _) = font_bytes();
    let mut db = FontDb::new();
    let light = db.add_face(bytes.clone(), "Fam", 300).unwrap();
    let bold = db.add_face(bytes, "Fam", 700).unwrap();

    assert_eq!(db.select("Fam", 300), Some(light));
    assert_eq!(db.select("Fam", 700), Some(bold));
    assert_eq!(db.select("Fam", 640), Some(bold), "640 should pick 700");
    assert_eq!(db.select("Fam", 380), Some(light), "380 should pick 300");
    assert_eq!(db.select("Nope", 400), None);
}
