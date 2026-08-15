//! Test patterns.
//!
//! Spec: `docs/specs/output-config.md` §8. These are diagnostic instruments,
//! not decoration — they are the only way an operator can *prove* an output's
//! configuration rather than trust it.
//!
//! Pure, like the rest of this crate: each generator fills a linear-light
//! **premultiplied** RGBA buffer. Conversion to a wire format, and to straight
//! alpha where a backend wants it, happens at the output boundary.

use crate::{premultiply, Range};

/// A linear-light, premultiplied RGBA frame in design space.
pub struct Frame {
    pub width: u32,
    pub height: u32,
    /// `width * height * 4`, in RGBA order, premultiplied, linear.
    pub rgba: Vec<f32>,
}

impl Frame {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            rgba: vec![0.0; (width as usize) * (height as usize) * 4],
        }
    }

    #[inline]
    pub fn put(&mut self, x: u32, y: u32, straight_rgb: [f32; 3], a: f32) {
        if x >= self.width || y >= self.height {
            return;
        }
        let i = ((y as usize) * (self.width as usize) + (x as usize)) * 4;
        let p = premultiply(straight_rgb, a);
        self.rgba[i] = p[0];
        self.rgba[i + 1] = p[1];
        self.rgba[i + 2] = p[2];
        self.rgba[i + 3] = a;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pattern {
    /// Ramp with marked 0% and 100% points. Proves `range` on a scope.
    Ramp,
    /// 75% colour bars. Proves `matrix` and `primaries` on a vectorscope.
    Bars,
    /// **The important one.** A soft-edged white shape on transparent.
    ///
    /// Keyed over a bright *and* a dark background, this is the only pattern
    /// that catches the classic broadcast CG bug: an incorrectly premultiplied
    /// antialiased edge. The error is invisible over one background and obvious
    /// over the other, which is exactly why it survives internal testing and
    /// shows up on air.
    SoftEdge,
    /// Alpha at 0, 25, 50, 75, 100%. Catches the *other* classic: a key that has
    /// been through a colour matrix. Chroma must be neutral at every step.
    AlphaStaircase,
    /// A moving 1-pixel vertical line. Proves frame timing — tearing or stutter
    /// is a dropped or late frame.
    MovingLine,
}

impl Pattern {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "ramp" => Pattern::Ramp,
            "bars" => Pattern::Bars,
            "softedge" => Pattern::SoftEdge,
            "alphastair" => Pattern::AlphaStaircase,
            "line" => Pattern::MovingLine,
            _ => return None,
        })
    }

    pub const ALL: &'static [&'static str] = &["ramp", "bars", "softedge", "alphastair", "line"];
}

/// Render `pattern` at `frame_no`. Only [`Pattern::MovingLine`] animates.
pub fn render(pattern: Pattern, w: u32, h: u32, frame_no: u64) -> Frame {
    match pattern {
        Pattern::Ramp => ramp(w, h),
        Pattern::Bars => bars(w, h),
        Pattern::SoftEdge => soft_edge(w, h),
        Pattern::AlphaStaircase => alpha_staircase(w, h),
        Pattern::MovingLine => moving_line(w, h, frame_no),
    }
}

/// Horizontal ramp, fully opaque, with black marks at exactly 0% and 100%.
///
/// The marks are what make this provable: on a scope they must land at the
/// range endpoints (64 and 940 in 10-bit narrow, 0 and 1023 full). A ramp
/// without marked reference points tells you nothing.
fn ramp(w: u32, h: u32) -> Frame {
    let mut f = Frame::new(w, h);
    let mark = (h / 10).max(4);
    for y in 0..h {
        for x in 0..w {
            let v = x as f32 / (w.saturating_sub(1)).max(1) as f32;
            // Top and bottom bands hold the two endpoints flat, so a scope has
            // something unambiguous to sit on.
            let v = if y < mark {
                0.0
            } else if y >= h - mark {
                1.0
            } else {
                v
            };
            f.put(x, y, [v, v, v], 1.0);
        }
    }
    f
}

/// 75% colour bars, in the classic order.
fn bars(w: u32, h: u32) -> Frame {
    const L: f32 = 0.75;
    let cols: [[f32; 3]; 8] = [
        [L, L, L],
        [L, L, 0.0],
        [0.0, L, L],
        [0.0, L, 0.0],
        [L, 0.0, L],
        [L, 0.0, 0.0],
        [0.0, 0.0, L],
        [0.0, 0.0, 0.0],
    ];
    let mut f = Frame::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let i = ((x as usize) * cols.len() / (w.max(1) as usize)).min(cols.len() - 1);
            f.put(x, y, cols[i], 1.0);
        }
    }
    f
}

/// A soft-edged white disc and a soft-edged bar on full transparency.
///
/// The edges use analytic coverage — not a blur — so the alpha gradient is
/// exactly what the renderer's own antialiasing will produce, and any fringing
/// downstream is the boundary's fault rather than the pattern's.
fn soft_edge(w: u32, h: u32) -> Frame {
    let mut f = Frame::new(w, h);
    let (cx, cy) = (w as f32 * 0.32, h as f32 * 0.5);
    let r = (h as f32) * 0.30;

    let bar_x0 = w as f32 * 0.58;
    let bar_x1 = w as f32 * 0.92;
    let bar_y0 = h as f32 * 0.38;
    let bar_y1 = h as f32 * 0.62;

    for y in 0..h {
        for x in 0..w {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);

            // Analytic coverage: 1px of falloff across the boundary.
            let d = ((px - cx).powi(2) + (py - cy).powi(2)).sqrt();
            let disc = (0.5 - (d - r)).clamp(0.0, 1.0);

            let inside = |v: f32, lo: f32, hi: f32| {
                ((v - lo) + 0.5)
                    .clamp(0.0, 1.0)
                    .min(((hi - v) + 0.5).clamp(0.0, 1.0))
            };
            let bar = inside(px, bar_x0, bar_x1) * inside(py, bar_y0, bar_y1);

            let a = disc.max(bar);
            if a > 0.0 {
                f.put(x, y, [1.0, 1.0, 1.0], a);
            }
        }
    }
    f
}

/// Five vertical bands of white at alpha 0, 25, 50, 75, 100%.
///
/// Encoded through the key path, chroma must read neutral at every band. Any
/// deviation means the key went through a colour matrix — the failure ADR 0012
/// §3 exists to prevent.
fn alpha_staircase(w: u32, h: u32) -> Frame {
    const STEPS: [f32; 5] = [0.0, 0.25, 0.5, 0.75, 1.0];
    let mut f = Frame::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let i = ((x as usize) * STEPS.len() / (w.max(1) as usize)).min(STEPS.len() - 1);
            f.put(x, y, [1.0, 1.0, 1.0], STEPS[i]);
        }
    }
    f
}

/// A 1-pixel white line sweeping left to right over an opaque dark field.
fn moving_line(w: u32, h: u32, frame_no: u64) -> Frame {
    let mut f = Frame::new(w, h);
    let lx = (frame_no % (w.max(1) as u64)) as u32;
    for y in 0..h {
        for x in 0..w {
            let on = x == lx;
            let v = if on { 1.0 } else { 0.05 };
            f.put(x, y, [v, v, v], 1.0);
        }
    }
    f
}

/// Encode a premultiplied linear frame to 8-bit **straight-alpha BGRA**.
///
/// This is the NDI boundary (`docs/specs/color-and-key.md` §5). Two things
/// happen here and both are easy to get wrong:
///
/// 1. **Unpremultiply.** NDI wants straight alpha; we hold premultiplied.
///    Getting the direction wrong produces fringing that is invisible over one
///    background and obvious over the other.
/// 2. **Encode linear → sRGB-ish transfer, once.** Compositing happened in
///    linear; this is the single encode at the boundary.
///
/// Alpha itself is **not** transfer-encoded — it is a coverage fraction, mapped
/// linearly, always.
pub fn to_bgra8_straight(f: &Frame, range: Range) -> Vec<u8> {
    let n = (f.width as usize) * (f.height as usize);
    let mut out = vec![0u8; n * 4];

    // 8-bit endpoints for the configured range.
    let (lo, hi) = match range {
        Range::Narrow => (16.0f32, 235.0f32),
        Range::Full => (0.0f32, 255.0f32),
    };

    for i in 0..n {
        let a = f.rgba[i * 4 + 3].clamp(0.0, 1.0);
        let straight =
            crate::unpremultiply([f.rgba[i * 4], f.rgba[i * 4 + 1], f.rgba[i * 4 + 2]], a);
        let enc = |v: f32| -> u8 {
            let e = linear_to_srgb(v.clamp(0.0, 1.0));
            (lo + e * (hi - lo)).round().clamp(0.0, 255.0) as u8
        };
        out[i * 4] = enc(straight[2]); // B
        out[i * 4 + 1] = enc(straight[1]); // G
        out[i * 4 + 2] = enc(straight[0]); // R
                                           // Alpha is coverage. Linear, full-range, never transfer-encoded.
        out[i * 4 + 3] = (a * 255.0).round() as u8;
    }
    out
}

/// The sRGB transfer function. Rec.709's OETF differs slightly in the toe; for
/// a computer-monitor confidence check over NDI this is the right curve, and
/// `docs/specs/color-and-key.md` names the transfer explicitly per output so
/// this is a choice rather than an accident.
#[inline]
fn linear_to_srgb(v: f32) -> f32 {
    if v <= 0.003_130_8 {
        12.92 * v
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn soft_edge_actually_has_soft_edges() {
        // If every pixel is 0 or 1 there is no antialiased edge to test the
        // premultiply boundary with, and the pattern is useless.
        let f = render(Pattern::SoftEdge, 320, 180, 0);
        let partial = f
            .rgba
            .chunks(4)
            .filter(|p| p[3] > 0.01 && p[3] < 0.99)
            .count();
        assert!(
            partial > 50,
            "expected a soft edge, found {partial} partial pixels"
        );
    }

    #[test]
    fn alpha_staircase_hits_its_five_steps() {
        let f = render(Pattern::AlphaStaircase, 500, 4, 0);
        let mut seen: Vec<u8> = f
            .rgba
            .chunks(4)
            .map(|p| (p[3] * 100.0).round() as u8)
            .collect();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen, vec![0, 25, 50, 75, 100]);
    }

    #[test]
    fn premultiplied_storage_never_exceeds_alpha() {
        // The defining property of premultiplied storage. If a channel exceeds
        // alpha, unpremultiplying at the boundary produces values above 1.0 and
        // the edges clip.
        for p in [Pattern::SoftEdge, Pattern::AlphaStaircase] {
            let f = render(p, 160, 90, 0);
            for px in f.rgba.chunks(4) {
                let a = px[3];
                for c in &px[..3] {
                    assert!(
                        *c <= a + 1e-5,
                        "premultiplied channel {c} exceeds alpha {a}"
                    );
                }
            }
        }
    }

    #[test]
    fn bgra_round_trip_preserves_alpha_and_is_opaque_where_it_should_be() {
        let f = render(Pattern::Bars, 64, 8, 0);
        let bytes = to_bgra8_straight(&f, Range::Full);
        assert_eq!(bytes.len(), 64 * 8 * 4);
        assert!(
            bytes.chunks(4).all(|p| p[3] == 255),
            "bars must be fully opaque"
        );
    }

    #[test]
    fn narrow_range_stays_legal() {
        let f = render(Pattern::Ramp, 256, 8, 0);
        let bytes = to_bgra8_straight(&f, Range::Narrow);
        for p in bytes.chunks(4) {
            for c in &p[..3] {
                assert!(
                    (16..=235).contains(c),
                    "value {c} outside 8-bit narrow range"
                );
            }
        }
    }

    #[test]
    fn transparent_pixels_unpremultiply_to_black_not_infinity() {
        let f = render(Pattern::AlphaStaircase, 500, 2, 0);
        let bytes = to_bgra8_straight(&f, Range::Full);
        for p in bytes.chunks(4) {
            if p[3] == 0 {
                assert_eq!(&p[..3], &[0, 0, 0]);
            }
        }
    }
}
