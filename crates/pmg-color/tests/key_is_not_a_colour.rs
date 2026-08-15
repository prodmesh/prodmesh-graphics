//! The one colour rule that is easiest to break and hardest to see.
//!
//! `docs/specs/color-and-key.md` §3: fill goes RGB→YCbCr through a matrix; the
//! key is a monotone alpha signal with neutral chroma and **no matrix**.
//! Running the key through the fill path is tempting because that code already
//! works, and the damage — soft edges subtly wrong — is nearly impossible to
//! see except on the switcher, over real video, on the day.

use pmg_color::{encode_key_10bit, encode_key_8bit, rgb_to_ycbcr_8bit, Matrix, Range};

#[test]
fn the_key_has_neutral_chroma_at_every_step() {
    for step in 0..=100 {
        let a = step as f32 / 100.0;
        for range in [Range::Narrow, Range::Full] {
            let (_, cb, cr) = encode_key_8bit(a, range);
            assert_eq!((cb, cr), (128, 128), "alpha {a} leaked chroma");
            let (_, cb10, cr10) = encode_key_10bit(a, range);
            assert_eq!(
                (cb10, cr10),
                (512, 512),
                "alpha {a} leaked chroma in 10-bit"
            );
        }
    }
}

#[test]
fn the_key_is_monotone_and_hits_its_endpoints() {
    assert_eq!(encode_key_8bit(0.0, Range::Narrow).0, 16);
    assert_eq!(encode_key_8bit(1.0, Range::Narrow).0, 235);
    assert_eq!(encode_key_8bit(0.0, Range::Full).0, 0);
    assert_eq!(encode_key_8bit(1.0, Range::Full).0, 255);
    assert_eq!(encode_key_10bit(0.0, Range::Narrow).0, 64);
    assert_eq!(encode_key_10bit(1.0, Range::Narrow).0, 940);

    let mut last = 0u8;
    for step in 0..=255 {
        let y = encode_key_8bit(step as f32 / 255.0, Range::Full).0;
        assert!(y >= last, "the key must never go backwards");
        last = y;
    }
}

#[test]
fn the_key_ignores_colour_entirely() {
    // The property that matters is not that the two paths disagree — for a
    // NEUTRAL GREY they legitimately agree, because Y of a grey *is* the grey
    // value and both map through the same 16..235 span. The first version of
    // this test asserted they differ, and failed at 126 == 126 on exactly that
    // coincidence.
    //
    // What actually distinguishes the key is that it does not depend on colour
    // at all: full alpha is full key whatever the fill happens to be, whereas
    // fill luma swings enormously with hue.
    let key_full = encode_key_8bit(1.0, Range::Narrow).0;
    assert_eq!(key_full, 235);

    for rgb in [
        [1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, 0.0, 1.0],
        [1.0, 1.0, 1.0],
    ] {
        let fill_luma = rgb_to_ycbcr_8bit(rgb, Matrix::Bt709, Range::Narrow).0;
        if rgb != [1.0, 1.0, 1.0] {
            assert_ne!(
                fill_luma, key_full,
                "a saturated {rgb:?} must not encode like full coverage — if it does, \
                 the key is picking up the matrix"
            );
        }
    }

    // And a red fill is far darker than full key, which is the whole point: a
    // fully opaque red graphic keys at 100%, not at 54%.
    let red_luma = rgb_to_ycbcr_8bit([1.0, 0.0, 0.0], Matrix::Bt709, Range::Narrow).0;
    assert!(red_luma < 100, "709 red luma is ~63, got {red_luma}");
}

#[test]
fn a_neutral_grey_has_neutral_chroma_in_every_matrix() {
    // Grey is the one colour every matrix agrees on. A matrix that fails this
    // is transposed.
    for m in [Matrix::Bt601, Matrix::Bt709, Matrix::Bt2020] {
        for v in [0.0, 0.25, 0.5, 0.75, 1.0] {
            let (_, cb, cr) = rgb_to_ycbcr_8bit([v, v, v], m, Range::Full);
            assert!((cb as i32 - 128).abs() <= 1, "{m:?} grey {v} → Cb {cb}");
            assert!((cr as i32 - 128).abs() <= 1, "{m:?} grey {v} → Cr {cr}");
        }
    }
}

#[test]
fn the_matrices_actually_differ_from_each_other() {
    // Pure green is where 601 and 709 disagree most, and it is the shot that
    // gets blamed on the camera. If these matched, the setting is decorative.
    let g601 = rgb_to_ycbcr_8bit([0.0, 1.0, 0.0], Matrix::Bt601, Range::Full);
    let g709 = rgb_to_ycbcr_8bit([0.0, 1.0, 0.0], Matrix::Bt709, Range::Full);
    let g2020 = rgb_to_ycbcr_8bit([0.0, 1.0, 0.0], Matrix::Bt2020, Range::Full);
    assert_ne!(g601.0, g709.0);
    assert_ne!(g709.0, g2020.0);
    // Green luma must follow Kg, DERIVED from the coefficients rather than
    // written from memory. Two earlier versions of this assertion had the
    // ordering wrong in two different ways — Kg is 0.587 (601), 0.678 (2020),
    // 0.7152 (709), so it is not the ordering anyone guesses.
    let kg = |m: Matrix| {
        let (kr, kb) = m.coefficients();
        1.0 - kr - kb
    };
    let mut by_kg = [Matrix::Bt601, Matrix::Bt709, Matrix::Bt2020];
    by_kg.sort_by(|a, b| kg(*a).partial_cmp(&kg(*b)).unwrap());
    let luma = |m: Matrix| rgb_to_ycbcr_8bit([0.0, 1.0, 0.0], m, Range::Full).0;
    assert!(
        luma(by_kg[0]) < luma(by_kg[1]) && luma(by_kg[1]) < luma(by_kg[2]),
        "green luma must rise with Kg: {:?}={} {:?}={} {:?}={}",
        by_kg[0],
        luma(by_kg[0]),
        by_kg[1],
        luma(by_kg[1]),
        by_kg[2],
        luma(by_kg[2])
    );
}

#[test]
fn narrow_range_luma_and_chroma_have_different_spans() {
    // Luma 16..235, chroma 16..240. One span for both is the classic
    // hand-rolled bug and it shifts every saturated colour slightly.
    let (y_white, _, _) = rgb_to_ycbcr_8bit([1.0, 1.0, 1.0], Matrix::Bt709, Range::Narrow);
    let (y_black, _, _) = rgb_to_ycbcr_8bit([0.0, 0.0, 0.0], Matrix::Bt709, Range::Narrow);
    assert_eq!((y_black, y_white), (16, 235));

    let (_, cb, _) = rgb_to_ycbcr_8bit([0.0, 0.0, 1.0], Matrix::Bt709, Range::Narrow);
    assert!(cb >= 236, "narrow chroma should reach ~240, got {cb}");
}
