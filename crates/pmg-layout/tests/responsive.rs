//! The responsive behaviour the layout model exists for.
//!
//! These assert **numbers**, not pixels. That is deliberate: a layout
//! regression should surface here as a moved rectangle, not as a subtly
//! different image that nobody looks at closely enough.
//!
//! All of it uses [`StubMeasure`], a fixed function of character count. Real
//! shaping arrives in M1c, and when it does these tests must keep passing —
//! their assertions are about *relationships* between rectangles, not about
//! absolute text widths.

use pmg_layout::{solve, StubMeasure, TextMeasure};
use pmg_scene::bind::Data;
use pmg_scene::fixture;

fn data(pairs: &[(&str, &str)]) -> Data {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn band_for(name: &str, title: &str) -> pmg_layout::Rect {
    let doc = fixture::lower_third();
    let solved = solve(
        &doc,
        &data(&[("name", name), ("title", title)]),
        &StubMeasure::default(),
    )
    .expect("layout");
    solved.rect("band").expect("band")
}

// ---------------------------------------------------------------------------
// Shrink and grow — the canonical broadcast responsive case
// ---------------------------------------------------------------------------

#[test]
fn the_band_grows_and_shrinks_with_the_name() {
    let short = band_for("Dan", "Lead Pastor");
    let medium = band_for("Pastor Dan Whitfield", "Lead Pastor");
    let long = band_for("Reverend Bartholomew Fotheringay", "Lead Pastor");

    assert!(
        short.w < medium.w && medium.w < long.w,
        "band width is not monotonic in name length: {} {} {}",
        short.w,
        medium.w,
        long.w
    );
}

#[test]
fn the_band_is_sized_by_its_widest_child_not_by_the_canvas() {
    // A content-sized box that silently filled its parent would look fine on a
    // 1920 canvas and be wrong the moment anything sat beside it.
    let doc = fixture::lower_third();
    let solved = solve(
        &doc,
        &data(&[("name", "Dan"), ("title", "X")]),
        &StubMeasure::default(),
    )
    .unwrap();
    let band = solved.rect("band").unwrap();
    let root = solved.rect("root").unwrap();

    assert!(
        band.w < root.w * 0.5,
        "band {} is nearly canvas-wide {}",
        band.w,
        root.w
    );
}

#[test]
fn the_band_always_contains_its_children() {
    // The failure this whole mechanism exists to avoid: a box that does not
    // wrap its content, so text hangs outside the background it needs.
    for (name, title) in [
        ("Dan", "Lead Pastor"),
        ("Pastor Dan Whitfield", "Lead Pastor"),
        (
            "Reverend Bartholomew Fotheringay",
            "Associate Pastor of Something Long",
        ),
        ("A", ""),
    ] {
        let doc = fixture::lower_third();
        let solved = solve(
            &doc,
            &data(&[("name", name), ("title", title)]),
            &StubMeasure::default(),
        )
        .unwrap();
        let band = solved.rect("band").unwrap();

        for id in ["name", "title"] {
            let Some(r) = solved.rect(id) else { continue };
            assert!(
                r.x >= band.x - 0.01
                    && r.y >= band.y - 0.01
                    && r.right() <= band.right() + 0.01
                    && r.bottom() <= band.bottom() + 0.01,
                "{id} {r:?} escapes band {band:?} for name={name:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// visible_if — removal, not hiding
// ---------------------------------------------------------------------------

#[test]
fn an_empty_title_is_removed_from_layout_entirely() {
    let doc = fixture::lower_third();
    let solved = solve(
        &doc,
        &data(&[("name", "Pastor Dan Whitfield"), ("title", "")]),
        &StubMeasure::default(),
    )
    .unwrap();

    // Not merely zero-sized — absent. A zero-sized box still participates in
    // gap and alignment, which would leave the band visibly too tall.
    assert!(
        solved.rect("title").is_none(),
        "an omitted node produced a rectangle"
    );
    assert!(solved.was_omitted("title"));
}

#[test]
fn the_band_collapses_when_the_title_goes_away() {
    let with = band_for("Pastor Dan Whitfield", "Lead Pastor");
    let without = band_for("Pastor Dan Whitfield", "");

    assert!(
        without.h < with.h,
        "band did not collapse: {} vs {}",
        without.h,
        with.h
    );
    // Width is driven by the NAME here, which did not change — so a collapse
    // must not narrow the band as a side effect.
    assert_eq!(without.w, with.w, "collapsing the title changed the width");
}

#[test]
fn whitespace_counts_as_empty() {
    // An operator who taps space in the title field must get the collapsed
    // layout, not a band with a blank line in it.
    let blank = band_for("Dan", "   ");
    let empty = band_for("Dan", "");
    assert_eq!(blank.h, empty.h);
}

#[test]
fn a_collapsed_band_stays_bottom_anchored() {
    // The root is bottom-justified, so a shorter band must keep its BOTTOM edge
    // in place and move its top down — not shrink upward from the baseline.
    let with = band_for("Dan", "Lead Pastor");
    let without = band_for("Dan", "");
    assert!(
        (with.bottom() - without.bottom()).abs() < 0.01,
        "bottom edge moved: {} vs {}",
        with.bottom(),
        without.bottom()
    );
    assert!(without.y > with.y, "a shorter band should start lower");
}

// ---------------------------------------------------------------------------
// Determinism — what spike S1 compares before any pixels exist
// ---------------------------------------------------------------------------

#[test]
fn layout_is_deterministic() {
    let doc = fixture::lower_third();
    let d = data(&[("name", "Pastor Dan Whitfield"), ("title", "Lead Pastor")]);
    let a = solve(&doc, &d, &StubMeasure::default()).unwrap();
    let b = solve(&doc, &d, &StubMeasure::default()).unwrap();

    assert_eq!(a.rects, b.rects, "layout is not deterministic");
    assert_eq!(a.text, b.text);
}

#[test]
fn bindings_are_resolved_once_and_carried_through() {
    // The renderer must never re-evaluate a binding — the solver already did it,
    // and doing it twice invites the two answers to differ.
    let doc = fixture::lower_third();
    let solved = solve(
        &doc,
        &data(&[("name", "  Pastor Dan  "), ("title", "Lead Pastor")]),
        &StubMeasure::default(),
    )
    .unwrap();

    // The fixture trims and ellipsizes, so the carried text is post-transform.
    assert_eq!(
        solved.text.get("name").map(String::as_str),
        Some("Pastor Dan")
    );
}

// ---------------------------------------------------------------------------
// The measurer boundary
// ---------------------------------------------------------------------------

#[test]
fn a_different_measurer_changes_the_layout() {
    // Proves the trait is actually load-bearing rather than decorative — if
    // swapping the measurer changed nothing, text would not be driving size and
    // none of the tests above would mean what they claim.
    #[derive(Clone, Copy)]
    struct Wide;
    impl TextMeasure for Wide {
        fn measure(
            &self,
            text: &str,
            font: &pmg_scene::doc::FontSpec,
            _max: Option<f32>,
            _max_lines: usize,
        ) -> (f32, f32) {
            (text.chars().count() as f32 * font.size * 2.0, font.size)
        }
    }

    let doc = fixture::lower_third();
    let d = data(&[("name", "Dan"), ("title", "X")]);
    let narrow = solve(&doc, &d, &StubMeasure::default()).unwrap();
    let wide = solve(&doc, &d, &Wide).unwrap();

    assert!(
        wide.rect("band").unwrap().w > narrow.rect("band").unwrap().w,
        "the text measurer does not drive layout"
    );
}

#[test]
fn max_width_constrains_a_very_long_name() {
    // The fixture caps the band at 1200. Without a cap, a pathological name
    // would run off the canvas rather than wrapping or clipping.
    let doc = fixture::lower_third();
    let solved = solve(
        &doc,
        &data(&[("name", &"M".repeat(400)), ("title", "")]),
        &StubMeasure::default(),
    )
    .unwrap();
    let band = solved.rect("band").unwrap();
    assert!(
        band.w <= 1200.0 + 0.01,
        "band exceeded its max_width: {}",
        band.w
    );
}
