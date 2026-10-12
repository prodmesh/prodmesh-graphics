//! 10-bit 4:2:2 YCbCr in DeckLink's `v210` packing — the node encoding SDI
//! itself, so the output's matrix and ranges are ours rather than the card's
//! (ADR 0012, `docs/specs/color-and-key.md`).
//!
//! **This is the reference.** The GPU pass in `pmg-render` does the same
//! arithmetic per pixel every frame and is tested against this; the node uses
//! this directly only for test patterns, which are static. One definition of
//! the numbers, two executions of it.
//!
//! The input is the renderer's working space: linear-light, premultiplied
//! RGBA. Two signals come out:
//!
//! - **Fill**: un-premultiply, apply the transfer function, re-premultiply —
//!   premultiplied over black is what a fill feed is — then R'G'B'→YCbCr
//!   through the configured matrix, into the configured range.
//! - **Key**: `Y = alpha`, linear, into the key's OWN range; chroma neutral.
//!   Never a matrix, never a transfer function. The key is not a colour.

use crate::{Matrix, Range, NEUTRAL_CHROMA_10};

/// Bytes per row of a `v210` frame: six pixels per 16 bytes, rows padded to a
/// multiple of 48 pixels (128 bytes). DeckLink's own formula; a row that is
/// merely "width / 6 * 16" is short for any width not a multiple of 48, and the
/// picture shears.
pub const fn v210_row_bytes(width: u32) -> usize {
    (width as usize).div_ceil(48) * 128
}

/// The code-value mapping for one output, 10-bit. Plain numbers so the GPU
/// pass can be handed exactly the same ones.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Levels {
    pub kr: f32,
    pub kb: f32,
    /// `Y = y_off + y_scale * Y'`
    pub y_off: f32,
    pub y_scale: f32,
    /// `C = 512 + c_scale * C'`, with C' in -0.5..0.5
    pub c_scale: f32,
    pub y_min: f32,
    pub y_max: f32,
    pub c_min: f32,
    pub c_max: f32,
    /// `K = k_off + k_scale * alpha`
    pub k_off: f32,
    pub k_scale: f32,
    pub k_min: f32,
    pub k_max: f32,
}

impl Levels {
    /// The mapping for a fill matrix and range, and an independent key range.
    ///
    /// Narrow is 64–940 luma and 64–960 chroma, and nothing leaves it — a
    /// switcher clips super-white anyway, and S9 measures that nothing does.
    /// Full is 0–1023 by definition but CLAMPED to 4–1019 here: 0–3 and
    /// 1020–1023 are SDI's timing-reference codes, and a pixel that lands on
    /// one is a corrupted line, not a bright one.
    pub fn new(matrix: Matrix, range: Range, key_range: Range) -> Self {
        let (kr, kb) = matrix.coefficients();
        let (y_off, y_scale, c_scale, y_min, y_max, c_min, c_max) = match range {
            Range::Narrow => (64.0, 876.0, 896.0, 64.0, 940.0, 64.0, 960.0),
            Range::Full => (0.0, 1023.0, 1023.0, 4.0, 1019.0, 4.0, 1019.0),
        };
        let (k_off, k_scale, k_min, k_max) = match key_range {
            Range::Narrow => (64.0, 876.0, 64.0, 940.0),
            Range::Full => (0.0, 1023.0, 4.0, 1019.0),
        };
        Self {
            kr,
            kb,
            y_off,
            y_scale,
            c_scale,
            y_min,
            y_max,
            c_min,
            c_max,
            k_off,
            k_scale,
            k_min,
            k_max,
        }
    }
}

/// The sRGB curve — the SAME one the 8-bit encode pass uses
/// (`pmg-render/src/scene.wgsl`, `linear_to_srgb`), so switching an output to
/// node-side YCbCr changes who does the matrix, not how bright the picture is.
/// BT.709's OETF differs in the toe; choosing it is a per-output transfer
/// setting the format does not have yet (color-and-key.md §2).
#[inline]
fn oetf(v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    if v <= 0.003_130_8 {
        12.92 * v
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

/// Round half up, the same on every executor. WGSL's `round` is half-to-even
/// and Rust's is half-away-from-zero; `floor(x + 0.5)` is the one both have.
#[inline]
fn code(v: f32, lo: f32, hi: f32) -> u32 {
    (v + 0.5).floor().clamp(lo, hi) as u32
}

/// One pixel's fill as (Y', Cb', Cr'), premultiplied over black. Y' in 0..1,
/// chroma in -0.5..0.5.
#[inline]
pub fn fill_components(px: [f32; 4], l: &Levels) -> (f32, f32, f32) {
    let a = px[3].clamp(0.0, 1.0);
    if a <= 0.0 {
        return (0.0, 0.0, 0.0);
    }
    // Un-premultiply, encode, re-premultiply. Encoding the premultiplied value
    // is wrong because the curve is not linear, and the error is largest at
    // the low alphas an antialiased edge is made of.
    let r = oetf(px[0] / a) * a;
    let g = oetf(px[1] / a) * a;
    let b = oetf(px[2] / a) * a;
    let y = l.kr * r + (1.0 - l.kr - l.kb) * g + l.kb * b;
    (
        y,
        (b - y) / (2.0 * (1.0 - l.kb)),
        (r - y) / (2.0 * (1.0 - l.kr)),
    )
}

/// Encode a linear, premultiplied RGBA frame (`width * height * 4` floats) as
/// a `v210` fill and a `v210` key.
pub fn encode(rgba: &[f32], width: u32, height: u32, l: &Levels) -> (Vec<u8>, Vec<u8>) {
    let row = v210_row_bytes(width);
    let mut fill = vec![0u8; row * height as usize];
    let mut key = vec![0u8; row * height as usize];
    let w = width as usize;
    let at = |x: usize, y: usize| -> [f32; 4] {
        if x >= w {
            // Padding past the picture is transparent black: black fill,
            // keyed out.
            return [0.0; 4];
        }
        let i = (y * w + x) * 4;
        [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
    };
    let mid = f32::from(NEUTRAL_CHROMA_10);
    for y in 0..height as usize {
        for block in 0..row / 16 {
            let mut ys = [0u32; 6];
            let mut cs = [(0u32, 0u32); 3];
            let mut ks = [0u32; 6];
            for pair in 0..3 {
                let x0 = block * 6 + pair * 2;
                let p0 = at(x0, y);
                let p1 = at(x0 + 1, y);
                let (y0, cb0, cr0) = fill_components(p0, l);
                let (y1, cb1, cr1) = fill_components(p1, l);
                ys[pair * 2] = code(l.y_off + l.y_scale * y0, l.y_min, l.y_max);
                ys[pair * 2 + 1] = code(l.y_off + l.y_scale * y1, l.y_min, l.y_max);
                // 4:2:2 by averaging the pair — a box filter, and the same one
                // the GPU pass uses.
                cs[pair] = (
                    code(mid + l.c_scale * (cb0 + cb1) * 0.5, l.c_min, l.c_max),
                    code(mid + l.c_scale * (cr0 + cr1) * 0.5, l.c_min, l.c_max),
                );
                for (k, p) in [p0, p1].iter().enumerate() {
                    ks[pair * 2 + k] =
                        code(l.k_off + l.k_scale * p[3].clamp(0.0, 1.0), l.k_min, l.k_max);
                }
            }
            let at_byte = y * row + block * 16;
            pack(&mut fill[at_byte..at_byte + 16], ys, cs);
            let neutral = (NEUTRAL_CHROMA_10 as u32, NEUTRAL_CHROMA_10 as u32);
            pack(&mut key[at_byte..at_byte + 16], ks, [neutral; 3]);
        }
    }
    (fill, key)
}

/// The four little-endian words of one six-pixel `v210` block.
///
/// ```text
/// w0: Cb0  Y0  Cr0     (bits 0-9, 10-19, 20-29)
/// w1: Y1   Cb2 Y2
/// w2: Cr2  Y3  Cb4
/// w3: Y4   Cr4 Y5
/// ```
pub fn pack(out: &mut [u8], y: [u32; 6], c: [(u32, u32); 3]) {
    let w = |a: u32, b: u32, c: u32| a | (b << 10) | (c << 20);
    let words = [
        w(c[0].0, y[0], c[0].1),
        w(y[1], c[1].0, y[2]),
        w(c[1].1, y[3], c[2].0),
        w(y[4], c[2].1, y[5]),
    ];
    for (i, word) in words.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_le_bytes());
    }
}

/// One six-pixel block's code values: six lumas, three (Cb, Cr) pairs.
pub type Block = ([u32; 6], [(u32, u32); 3]);

/// Unpack one block back into its code values — for tests and for a capture
/// path that wants to measure what went out.
pub fn unpack(block: &[u8]) -> Block {
    let word = |i: usize| u32::from_le_bytes(block[i * 4..i * 4 + 4].try_into().unwrap());
    let f = |v: u32, n: u32| (v >> (10 * n)) & 0x3ff;
    let (w0, w1, w2, w3) = (word(0), word(1), word(2), word(3));
    (
        [f(w0, 1), f(w1, 0), f(w1, 2), f(w2, 1), f(w3, 0), f(w3, 2)],
        [
            (f(w0, 0), f(w0, 2)),
            (f(w1, 1), f(w2, 0)),
            (f(w2, 2), f(w3, 1)),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encode a 6x1 frame of one colour and return the first block.
    fn one(px: [f32; 4], l: &Levels) -> (Block, Block) {
        let frame: Vec<f32> = (0..6).flat_map(|_| px).collect();
        let (f, k) = encode(&frame, 6, 1, l);
        (unpack(&f[..16]), unpack(&k[..16]))
    }

    #[test]
    fn rows_are_padded_to_48_pixels() {
        assert_eq!(v210_row_bytes(1920), 5120);
        assert_eq!(v210_row_bytes(1280), 3456, "1280 is not a multiple of 48");
        assert_eq!(v210_row_bytes(720), 1920);
        assert_eq!(v210_row_bytes(3840), 10240);
    }

    #[test]
    fn narrow_black_and_white_land_on_64_and_940() {
        let l = Levels::new(Matrix::Bt709, Range::Narrow, Range::Narrow);
        let ((y, c), _) = one([1.0, 1.0, 1.0, 1.0], &l);
        assert_eq!(y, [940; 6]);
        assert_eq!(c, [(512, 512); 3], "white has no colour");
        let ((y, c), _) = one([0.0, 0.0, 0.0, 1.0], &l);
        assert_eq!(y, [64; 6]);
        assert_eq!(c, [(512, 512); 3]);
    }

    #[test]
    fn bt709_red_is_the_textbook_value() {
        // 100% red, 10-bit narrow BT.709: Y 250, Cb 409, Cr 960.
        let l = Levels::new(Matrix::Bt709, Range::Narrow, Range::Narrow);
        let ((y, c), _) = one([1.0, 0.0, 0.0, 1.0], &l);
        assert_eq!(y[0], 250);
        assert_eq!(c[0], (409, 960));
    }

    #[test]
    fn the_matrix_is_applied_not_assumed() {
        let red = [1.0, 0.0, 0.0, 1.0];
        let ((y709, _), _) = one(
            red,
            &Levels::new(Matrix::Bt709, Range::Narrow, Range::Narrow),
        );
        let ((y601, _), _) = one(
            red,
            &Levels::new(Matrix::Bt601, Range::Narrow, Range::Narrow),
        );
        // 601 red is Y 326 (64 + 876 * 0.299).
        assert_eq!(y601[0], 326);
        assert_ne!(y709[0], y601[0]);
    }

    #[test]
    fn full_range_avoids_the_timing_reference_codes() {
        let l = Levels::new(Matrix::Bt709, Range::Full, Range::Full);
        let ((y, _), (k, _)) = one([1.0, 1.0, 1.0, 1.0], &l);
        assert_eq!(y[0], 1019, "1023 would be a TRS code, not white");
        assert_eq!(k[0], 1019);
        let ((y, c), (k, _)) = one([0.0, 0.0, 0.0, 0.0], &l);
        assert_eq!(y[0], 4);
        assert_eq!(k[0], 4);
        assert_eq!(c[0], (512, 512));
    }

    #[test]
    fn the_key_is_alpha_in_its_own_range_with_neutral_chroma() {
        // Fill narrow, key FULL — the two are independent (output-config §4).
        let l = Levels::new(Matrix::Bt709, Range::Narrow, Range::Full);
        for (alpha, want) in [(0.0, 4), (0.25, 256), (0.5, 512), (1.0, 1019)] {
            // A saturated colour, so a key that leaked colour would show it.
            let (_, (k, kc)) = one([0.0, alpha, 0.0, alpha], &l);
            assert_eq!(k[0], want, "alpha {alpha}");
            assert_eq!(
                kc,
                [(512, 512); 3],
                "the key carries no colour at alpha {alpha}"
            );
        }
    }

    #[test]
    fn a_transparent_pixel_is_black_fill_and_keyed_out() {
        let l = Levels::new(Matrix::Bt709, Range::Narrow, Range::Narrow);
        let ((y, c), (k, _)) = one([0.0; 4], &l);
        assert_eq!((y[0], c[0], k[0]), (64, (512, 512), 64));
    }

    #[test]
    fn half_covered_white_is_half_bright_fill() {
        // Premultiplied over black: a 50% edge of white type is grey fill with
        // a 50% key, NOT full white — that is what makes the keyer's mix right.
        let l = Levels::new(Matrix::Bt709, Range::Narrow, Range::Narrow);
        let ((y, _), (k, _)) = one([0.5, 0.5, 0.5, 0.5], &l);
        assert_eq!(y[0], 502, "64 + 876 * 0.5");
        assert_eq!(k[0], 502);
    }

    #[test]
    fn padding_past_the_picture_is_keyed_out_black() {
        // 1280 wide pads each row to 1296 pixels; the last 16 must not be
        // whatever the buffer held.
        let l = Levels::new(Matrix::Bt709, Range::Narrow, Range::Narrow);
        let frame = vec![1.0f32; 1280 * 4];
        let (f, k) = encode(&frame, 1280, 1, &l);
        let (y, _) = unpack(&f[3440..3456]);
        let (ky, _) = unpack(&k[3440..3456]);
        assert_eq!(y, [64; 6]);
        assert_eq!(ky, [64; 6]);
    }

    #[test]
    fn pack_and_unpack_agree() {
        let mut b = [0u8; 16];
        let y = [1, 2, 3, 1021, 512, 1023];
        let c = [(4, 5), (6, 7), (1000, 1)];
        pack(&mut b, y, [c[0], c[1], c[2]]);
        assert_eq!(unpack(&b), (y, [c[0], c[1], c[2]]));
    }
}
