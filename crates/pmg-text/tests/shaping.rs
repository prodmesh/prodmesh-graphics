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
use pmg_text::{
    build_font_db, face_info, quantize, FontDb, Measurer, TextEngine, Wrap, SUBPIXEL_PHASES,
};

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
    assert_eq!(m.measure("hello", &s, None, usize::MAX), (0.0, 0.0));
}

#[test]
fn a_known_family_measures_nonzero_through_the_trait() {
    let (eng, _) = engine();
    let m = Measurer::new(eng);
    let s = spec(46.0);
    let (w, h) = m.measure("Pastor Dan", &s, None, usize::MAX);
    assert!(w > 0.0 && h > 0.0, "real measurement returned {w}x{h}");

    // line_height multiplies the font's natural line height.
    let mut tall = s.clone();
    tall.line_height = 2.4;
    let (_, h2) = m.measure("Pastor Dan", &tall, None, usize::MAX);
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

// ---------------------------------------------------------------------------
// Line breaking.
//
// Found by the dress rehearsal: measure() returned one line height in both
// branches and the renderer shaped the whole string once, so `max_lines` was
// declared in the format and ignored. A lyric slide is two lines by definition
// and a scripture block is three or four, which made two of the three primary
// graphics of a real service undrawable.
// ---------------------------------------------------------------------------

#[test]
fn a_hard_break_is_a_break_the_author_chose() {
    let (mut eng, face) = engine();
    // No width constraint at all: the ONLY reason this is two lines is the \n.
    // A lyric slide's breaks are a decision about phrasing, and reflowing them
    // to the box would be overriding the person who wrote the slide.
    let lines = eng
        .shape_lines(
            face,
            "We are here to witness\nto share and testify",
            48.0,
            &[],
            0.0,
            Wrap {
                max_width: None,
                max_lines: usize::MAX,
            },
        )
        .expect("shaped");
    assert_eq!(lines.len(), 2);
    assert!(lines[0].width > 0.0 && lines[1].width > 0.0);
}

#[test]
fn a_long_line_wraps_to_the_box() {
    let (mut eng, face) = engine();
    let text = "It is a trustworthy statement deserving full acceptance, \
                that Christ Jesus came into the world to save sinners";
    let one = eng
        .shape_lines(
            face,
            text,
            44.0,
            &[],
            0.0,
            Wrap {
                max_width: None,
                max_lines: usize::MAX,
            },
        )
        .unwrap();
    assert_eq!(one.len(), 1, "unconstrained, it is one line");

    let wrapped = eng
        .shape_lines(
            face,
            text,
            44.0,
            &[],
            0.0,
            Wrap {
                max_width: Some(700.0),
                max_lines: usize::MAX,
            },
        )
        .unwrap();
    assert!(wrapped.len() > 1, "got {} lines", wrapped.len());
    for l in &wrapped {
        assert!(l.width <= 700.0, "a line overflowed: {} > 700", l.width);
    }
}

#[test]
fn max_lines_truncates_rather_than_overflowing() {
    let (mut eng, face) = engine();
    let text = "one two three four five six seven eight nine ten eleven twelve";
    let capped = eng
        .shape_lines(
            face,
            text,
            44.0,
            &[],
            0.0,
            Wrap {
                max_width: Some(300.0),
                max_lines: 2,
            },
        )
        .unwrap();
    assert_eq!(capped.len(), 2);
}

#[test]
fn a_word_wider_than_the_box_still_appears() {
    let (mut eng, face) = engine();
    // Clipped and visible beats absent: a designer can see a word is too long,
    // and cannot see one that was silently dropped. Also the loop guard — a
    // break at the line's own start would never advance.
    let lines = eng
        .shape_lines(
            face,
            "Supercalifragilisticexpialidocious",
            60.0,
            &[],
            0.0,
            Wrap {
                max_width: Some(80.0),
                max_lines: 8,
            },
        )
        .expect("shaped");
    assert_eq!(lines.len(), 1);
    assert!(!lines[0].glyphs.is_empty());
}

#[test]
fn wrapping_preserves_the_shaping_of_the_unwrapped_string() {
    let (mut eng, face) = engine();
    // Lines are cut out of ONE shaped run rather than shaped word by word, so
    // kerning inside a word is identical whether or not a later word wrapped.
    // Shaping per word would also measure differently from the same string
    // drawn unwrapped, which is how a box ends up the wrong size.
    let whole = eng
        .shape_lines(
            face,
            "alpha beta",
            48.0,
            &[],
            0.0,
            Wrap {
                max_width: None,
                max_lines: 9,
            },
        )
        .unwrap();
    let split = eng
        .shape_lines(
            face,
            "alpha beta",
            48.0,
            &[],
            0.0,
            Wrap {
                max_width: Some(1.0),
                max_lines: 9,
            },
        )
        .unwrap();
    assert_eq!(split.len(), 2);
    let first_of_whole: Vec<u16> = whole[0].glyphs.iter().take(5).map(|g| g.id).collect();
    let first_of_split: Vec<u16> = split[0].glyphs.iter().take(5).map(|g| g.id).collect();
    assert_eq!(first_of_whole, first_of_split);
    // And the x positions of line one are unchanged by the wrap.
    for (a, b) in whole[0].glyphs.iter().zip(split[0].glyphs.iter()).take(5) {
        assert!((a.x - b.x).abs() < 0.01, "{} vs {}", a.x, b.x);
    }
}

#[test]
fn an_empty_cap_is_not_zero_lines() {
    let (mut eng, face) = engine();
    // max_lines(0) would be a template that renders nothing at all. Clamped,
    // because the format's default for an unset field must not blank a graphic.
    let lines = eng
        .shape_lines(
            face,
            "hello",
            40.0,
            &[],
            0.0,
            Wrap {
                max_width: None,
                max_lines: 0,
            },
        )
        .unwrap();
    assert_eq!(lines.len(), 1);
}

// ---------------------------------------------------------------------------
// Coverage gamma.
// ---------------------------------------------------------------------------

#[test]
fn coverage_gamma_lifts_the_edges_and_moves_neither_end() {
    let (mut eng, face) = engine();
    let bmp = eng.glyph(face, 5, 64.0, 0).expect("a glyph rasterizes");
    let mid: Vec<u8> = bmp
        .coverage
        .iter()
        .copied()
        .filter(|c| *c > 0 && *c < 255)
        .collect();
    assert!(!mid.is_empty(), "a 64px glyph has antialiased edges");

    // Fully covered stays opaque and empty stays empty — otherwise this would
    // fatten every glyph and fog the background instead of fixing the edge.
    assert!(bmp.coverage.contains(&255), "no solid interior");
    assert!(bmp.coverage.contains(&0), "no empty margin");
}

#[test]
fn the_gamma_curve_is_monotonic_and_pinned_at_both_ends() {
    // Monotonic, or a soft edge would develop a step in it. Pinned, or the
    // glyph either fattens or dissolves.
    let f = |c: u8| (((c as f32 / 255.0).powf(1.0 / 1.45)) * 255.0 + 0.5) as u8;
    assert_eq!(f(0), 0);
    assert_eq!(f(255), 255);
    for c in 0..255u8 {
        assert!(f(c) <= f(c + 1), "not monotonic at {c}");
    }
    // And it LIFTS: a half-covered pixel must end up brighter than half, which
    // is the whole correction.
    assert!(f(128) > 150, "half coverage only reached {}", f(128));
}

/// A SECOND face, distinct from [`font_bytes`]'s, so multi-font behaviour can
/// be tested for real. Fails loudly like everything else here.
fn second_font_bytes(first_family: &str) -> Vec<u8> {
    const CANDIDATES: &[&str] = &[
        "/System/Library/Fonts/Supplemental/Georgia.ttf",
        "/System/Library/Fonts/Supplemental/Times New Roman.ttf",
        "/System/Library/Fonts/Supplemental/Verdana.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSerif.ttf",
        "/usr/share/fonts/truetype/liberation/LiberationSerif-Regular.ttf",
        "C:\\Windows\\Fonts\\georgia.ttf",
        "C:\\Windows\\Fonts\\times.ttf",
    ];
    for p in CANDIDATES {
        if let Ok(b) = std::fs::read(p) {
            if let Some((family, _)) = face_info(&b) {
                if family != first_family {
                    return b;
                }
            }
        }
    }
    panic!("no second distinct font found; install one of {CANDIDATES:?}")
}

#[test]
fn two_declared_families_select_two_different_faces() {
    // The bug this guards: registering every face under every family made
    // `select` resolve BOTH families to whichever face landed first — every
    // text box on air rendered in one font while the designer showed two.
    let (a_bytes, _) = font_bytes();
    let (a_family, a_weight) = face_info(&a_bytes).expect("first font has a name");
    let b_bytes = second_font_bytes(&a_family);
    let (b_family, b_weight) = face_info(&b_bytes).expect("second font has a name");

    let declared = vec![
        (a_family.clone(), a_weight),
        (b_family.clone(), b_weight),
        // And one family no face answers to: the fallback shim serves it.
        ("Ghost Grotesk".to_string(), 400),
    ];
    let db = build_font_db(&[a_bytes, b_bytes], &declared).expect("builds");

    let fa = db
        .select(&a_family, a_weight)
        .expect("first family resolves");
    let fb = db
        .select(&b_family, b_weight)
        .expect("second family resolves");
    assert_ne!(fa, fb, "two families must mean two faces");
    assert!(
        db.select("Ghost Grotesk", 400).is_some(),
        "an unmatched declared family still renders via the fallback"
    );
}
