//! Which buffer carries which convention.
//!
//! This has been wrong twice, and both times it looked like something else. The
//! first version unpremultiplied for a fill+key pair, which brightens every
//! soft edge on a keyer. The second sent STRAIGHT colour on a fill-only port —
//! and since SDI drops the alpha byte, a 10%-covered edge of white type arrived
//! as full white, so the antialiasing on the card looked broken while the same
//! frame over NDI looked perfect.

use pmg_out_decklink::build_frames;

/// One pixel of white at 10% coverage, premultiplied — a typical soft edge.
const EDGE_A: u8 = 26;
fn edge() -> Vec<u8> {
    vec![EDGE_A, EDGE_A, EDGE_A, EDGE_A]
}

#[test]
fn a_fill_only_port_carries_premultiplied_colour() {
    let (mut fill, mut key) = (vec![0u8; 4], vec![0u8; 4]);
    build_frames(&edge(), &mut fill, &mut key, false, false);

    // BGRA, so channel 0 is blue — white makes them equal, which is the point:
    // the value must still be 26, not 255. SDI discards alpha, so whatever is
    // in the colour channels IS what reaches the screen.
    assert_eq!(&fill[..3], &[EDGE_A, EDGE_A, EDGE_A], "edge was brightened");
    assert_eq!(fill[3], 255, "alpha must be opaque; an accidental 0 reads as a transparent frame");
}

#[test]
fn embedded_alpha_carries_straight_colour() {
    let (mut fill, mut key) = (vec![0u8; 4], vec![0u8; 4]);
    build_frames(&edge(), &mut fill, &mut key, false, true);

    // Only correct when something downstream actually keeps the fourth
    // channel — a capture device, or a card in a keying mode.
    assert_eq!(&fill[..3], &[255, 255, 255], "straight colour should be full white");
    assert_eq!(fill[3], EDGE_A, "coverage travels in alpha here");
}

#[test]
fn a_pair_sends_premultiplied_fill_and_a_monotone_key() {
    let (mut fill, mut key) = (vec![0u8; 4], vec![0u8; 4]);
    build_frames(&edge(), &mut fill, &mut key, true, false);

    // A downstream keyer computes bg*(1-key) + fill, so the fill must already
    // be multiplied by coverage. Sending straight fill brightens every soft
    // edge — the bright-fringe failure.
    assert_eq!(&fill[..3], &[EDGE_A, EDGE_A, EDGE_A]);
    assert_eq!(fill[3], 255);

    // The key is coverage replicated into all three channels, opaque. Never the
    // fill's colour, and never through a colour matrix.
    assert_eq!(key, vec![EDGE_A, EDGE_A, EDGE_A, 255]);
}

#[test]
fn a_transparent_pixel_is_black_rather_than_stale() {
    let (mut fill, mut key) = (vec![0u8; 4], vec![0u8; 4]);
    build_frames(&[9, 9, 9, 0], &mut fill, &mut key, false, true);
    // Unpremultiplying zero coverage would divide by zero; leaving the colour
    // behind shows up as a halo the moment a keyer softens the edge.
    assert_eq!(&fill[..3], &[0, 0, 0]);
}

#[test]
fn an_opaque_pixel_is_the_same_either_way() {
    // Where coverage is 1 the two conventions agree, which is why this bug
    // hides in the interior of every glyph and lives only at its edges.
    for straight in [false, true] {
        let (mut fill, mut key) = (vec![0u8; 4], vec![0u8; 4]);
        build_frames(&[10, 20, 30, 255], &mut fill, &mut key, false, straight);
        assert_eq!(&fill[..3], &[30, 20, 10], "BGRA order, straight={straight}");
    }
}
