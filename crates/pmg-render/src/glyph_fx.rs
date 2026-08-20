//! Derived glyph bitmaps for text effects — outline rings and shadow blurs.
//!
//! Built at ARM, never per frame: the per-pixel work happens once here and the
//! GPU then draws the results exactly like any other glyph
//! (`docs/INTEGRATION-NOTES.md` records what happens to a frame budget when
//! per-pixel work is done per frame).
//!
//! Everything is integer-and-f32 CPU arithmetic on coverage bytes, identical
//! on native and wasm — these bitmaps land in the atlas, and the atlas is part
//! of what ADR 0001 promises renders the same everywhere.
//!
//! # Why the outline is OUTSIDE the glyph
//!
//! A box stroke draws inside its edge, where there is room. A glyph has no
//! room: an inside stroke on broadcast-size text eats the counters and thins
//! every stem — which is why every CG in the industry draws text outlines
//! outward. The layout box is unchanged either way; like a shadow, the ring is
//! ink past the geometry, not geometry.

/// A derived bitmap, larger than its source, with its placement offset.
pub struct FxBitmap {
    /// Coverage bytes, `width * height`.
    pub bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// How far the top-left moved, in pixels (negative: up and left).
    pub offset: i32,
}

/// The disk footprint for a dilation radius, with an antialiased rim.
///
/// Weight is the linear coverage ramp `r + 0.5 - distance`, the same coverage
/// function the box SDF uses — so a ring's outer edge antialiases the way
/// every other edge in this renderer does.
fn disk(radius: f32) -> (i32, Vec<(i32, i32, f32)>) {
    let r = radius.ceil() as i32;
    let mut taps = Vec::new();
    for dy in -r..=r {
        for dx in -r..=r {
            let dist = ((dx * dx + dy * dy) as f32).sqrt();
            let w = (radius + 0.5 - dist).clamp(0.0, 1.0);
            if w > 0.0 {
                taps.push((dx, dy, w));
            }
        }
    }
    (r, taps)
}

/// Dilate coverage by `radius` and subtract the source: the outline ring.
///
/// Subtraction of coverages is how the box shader builds its bands, applied
/// here on the CPU: where the glyph's own antialiased edge fades in, the ring
/// fades out by exactly the same amount, so ring + fill composite to the
/// dilated silhouette with no seam.
pub fn outline_ring(cov: &[u8], w: u32, h: u32, radius: f32) -> FxBitmap {
    let (r, taps) = disk(radius);
    let (nw, nh) = (w + 2 * r as u32, h + 2 * r as u32);
    let mut dilated = vec![0u8; (nw * nh) as usize];
    for sy in 0..h as i32 {
        for sx in 0..w as i32 {
            let c = cov[(sy * w as i32 + sx) as usize];
            if c == 0 {
                continue;
            }
            for (dx, dy, wt) in &taps {
                let (x, y) = (sx + dx + r, sy + dy + r);
                let v = (c as f32 * wt) as u8;
                let d = &mut dilated[(y * nw as i32 + x) as usize];
                *d = (*d).max(v);
            }
        }
    }
    // Knock the glyph itself back out, so what remains is only the band the
    // stroke colour owns.
    for sy in 0..h {
        for sx in 0..w {
            let src = cov[(sy * w + sx) as usize];
            let d = &mut dilated[((sy + r as u32) * nw + sx + r as u32) as usize];
            *d = d.saturating_sub(src);
        }
    }
    FxBitmap {
        bytes: dilated,
        width: nw,
        height: nh,
        offset: -r,
    }
}

/// Dilate coverage by `radius` without the knockout: the full silhouette.
///
/// A shadow with an outline shadows the OUTLINED shape — the ink the viewer
/// sees — not the bare glyph inside it.
pub fn dilate(cov: &[u8], w: u32, h: u32, radius: f32) -> FxBitmap {
    let mut fx = outline_ring(cov, w, h, radius);
    let r = (-fx.offset) as u32;
    for sy in 0..h {
        for sx in 0..w {
            let src = cov[(sy * w + sx) as usize];
            let d = &mut fx.bytes[((sy + r) * fx.width + sx + r) as usize];
            *d = (*d).max(src);
        }
    }
    fx
}

/// Soften coverage with three box passes — the standard gaussian
/// approximation, in exact integer arithmetic.
///
/// `blur` is the softness in pixels, matching the box shadow's meaning of the
/// same field: roughly how far the ramp extends past the silhouette.
pub fn soften(cov: &[u8], w: u32, h: u32, blur: f32) -> FxBitmap {
    // Radius per pass, chosen so three passes reach about `blur` past the
    // edge. At least 1 so a small blur still softens rather than no-ops.
    let rb = ((blur / 3.0).round() as i32).max(1);
    let pad = 3 * rb;
    let (nw, nh) = (w as i32 + 2 * pad, h as i32 + 2 * pad);
    let mut buf = vec![0u16; (nw * nh) as usize];
    for sy in 0..h as i32 {
        for sx in 0..w as i32 {
            buf[((sy + pad) * nw + sx + pad) as usize] = cov[(sy * w as i32 + sx) as usize] as u16;
        }
    }
    let norm = (2 * rb + 1) as u32;
    for _ in 0..3 {
        // Horizontal then vertical, sliding-window sums.
        let mut next = vec![0u16; buf.len()];
        for y in 0..nh {
            let row = (y * nw) as usize;
            let mut acc: u32 = 0;
            for x in -rb..=rb {
                acc += buf[row + x.clamp(0, nw - 1) as usize] as u32;
            }
            for x in 0..nw {
                next[row + x as usize] = (acc / norm) as u16;
                let leave = (x - rb).clamp(0, nw - 1) as usize;
                let enter = (x + rb + 1).clamp(0, nw - 1) as usize;
                acc = acc + buf[row + enter] as u32 - buf[row + leave] as u32;
            }
        }
        buf = next;
        let mut next = vec![0u16; buf.len()];
        for x in 0..nw {
            let mut acc: u32 = 0;
            for y in -rb..=rb {
                acc += buf[(y.clamp(0, nh - 1) * nw + x) as usize] as u32;
            }
            for y in 0..nh {
                next[(y * nw + x) as usize] = (acc / norm) as u16;
                let leave = (y - rb).clamp(0, nh - 1);
                let enter = (y + rb + 1).clamp(0, nh - 1);
                acc = acc + buf[(enter * nw + x) as usize] as u32
                    - buf[(leave * nw + x) as usize] as u32;
            }
        }
        buf = next;
    }
    FxBitmap {
        bytes: buf.iter().map(|&v| v.min(255) as u8).collect(),
        width: nw as u32,
        height: nh as u32,
        offset: -pad,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An 8x8 solid square, the simplest glyph.
    fn square() -> (Vec<u8>, u32, u32) {
        (vec![255u8; 64], 8, 8)
    }

    #[test]
    fn the_ring_is_outside_and_the_interior_is_empty() {
        let (cov, w, h) = square();
        let fx = outline_ring(&cov, w, h, 2.0);
        assert_eq!(fx.offset, -2);
        assert_eq!((fx.width, fx.height), (12, 12));
        // Just outside the old edge: solid ring.
        assert_eq!(fx.bytes[(1 * 12 + 6) as usize], 255);
        // Dead centre of the glyph: knocked out entirely.
        assert_eq!(fx.bytes[(6 * 12 + 6) as usize], 0);
        // Beyond the radius: nothing.
        assert_eq!(fx.bytes[0], 0);
    }

    #[test]
    fn ring_plus_fill_reconstructs_the_silhouette_without_a_seam() {
        let (cov, w, h) = square();
        let ring = outline_ring(&cov, w, h, 2.0);
        let solid = dilate(&cov, w, h, 2.0);
        for y in 0..12u32 {
            for x in 0..12u32 {
                let r = ring.bytes[(y * 12 + x) as usize] as u16;
                let f = if (2..10).contains(&x) && (2..10).contains(&y) {
                    cov[((y - 2) * 8 + (x - 2)) as usize] as u16
                } else {
                    0
                };
                assert_eq!(
                    r + f,
                    solid.bytes[(y * 12 + x) as usize] as u16,
                    "seam at {x},{y}"
                );
            }
        }
    }

    #[test]
    fn soften_spreads_ink_past_the_edge_and_keeps_it_off_the_border() {
        let (cov, w, h) = square();
        let fx = soften(&cov, w, h, 4.0);
        let c = fx.bytes[((fx.height / 2) * fx.width + fx.width / 2) as usize];
        assert!(c > 200, "centre stays near solid, got {c}");
        // Past the old edge there is now ink; at the bitmap border it has
        // faded out (the pad covered the whole support).
        let past_edge = fx.bytes[((fx.height / 2) * fx.width + 2) as usize];
        assert!(past_edge > 0, "blur reaches past the edge");
        assert_eq!(fx.bytes[0], 0, "support fits inside the pad");
    }

    #[test]
    fn identical_inputs_give_identical_bytes() {
        // The atlas is part of what ADR 0001 promises is the same everywhere;
        // these must be pure functions of their inputs.
        let (cov, w, h) = square();
        assert_eq!(
            outline_ring(&cov, w, h, 2.5).bytes,
            outline_ring(&cov, w, h, 2.5).bytes
        );
        assert_eq!(soften(&cov, w, h, 3.0).bytes, soften(&cov, w, h, 3.0).bytes);
    }
}
