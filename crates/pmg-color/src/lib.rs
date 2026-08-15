//! Colour, range, and the key channel.
//!
//! Spec: `docs/specs/color-and-key.md`. Decision: ADR 0012.
//!
//! This crate is in the **pure set**: no I/O, no platform, no tokio, no vendor
//! SDKs. It builds for `wasm32-unknown-unknown`, and that build is what enforces
//! the rule (ADR 0004).
//!
//! The working space everywhere upstream of here is **linear-light,
//! premultiplied RGBA16F**. This crate is the only place a value is encoded for
//! output, and it happens exactly once.

#![forbid(unsafe_code)]

/// Signal range. `narrow` is what a switcher expects; some plants want a
/// full-range key, which is why fill and key carry this independently
/// (`docs/specs/output-config.md` §4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Range {
    Narrow,
    Full,
}

/// Neutral chroma in 10-bit. Not a colour — the absence of one.
pub const NEUTRAL_CHROMA_10: u16 = 512;

/// Encode the key plane, 10-bit.
///
/// **The key is not a colour.** It never goes through a colour matrix, never
/// through a transfer function, and never picks up primaries. Alpha is a
/// coverage fraction, mapped linearly to the code-value range, with neutral
/// chroma at every step.
///
/// Running the key through the fill path — which is tempting, because that code
/// already works — mixes a nonexistent chroma into the luma and shifts the alpha
/// values slightly. The result is a key whose soft edges are subtly wrong, which
/// makes the downstream keyer's soft edges wrong with it. It is nearly
/// impossible to see except on the switcher, over real video, on the day.
///
/// Returns `(Y, Cb, Cr)`.
pub fn encode_key_10bit(alpha_linear: f32, range: Range) -> (u16, u16, u16) {
    let a = alpha_linear.clamp(0.0, 1.0);
    let y = match range {
        Range::Narrow => 64.0 + a * (940.0 - 64.0),
        Range::Full => a * 1023.0,
    };
    (y.round() as u16, NEUTRAL_CHROMA_10, NEUTRAL_CHROMA_10)
}

/// Map a normalised [0,1] luma to 10-bit code values for the given range.
/// Values outside the range are clamped, never wrapped.
pub fn map_luma_10bit(v: f32, range: Range) -> u16 {
    let v = v.clamp(0.0, 1.0);
    let out = match range {
        Range::Narrow => 64.0 + v * (940.0 - 64.0),
        Range::Full => v * 1023.0,
    };
    out.round() as u16
}

/// Undo premultiplication at an output boundary that wants straight alpha
/// (NDI's RGBA, DeckLink's 8-bit BGRA embedded-alpha modes).
///
/// Internally everything is premultiplied; this is the only direction the
/// conversion happens, and only at the boundary. Getting it backwards produces
/// fringing — dark over a bright background, bright over a dark one — which is
/// exactly what the soft-edge test pattern exists to catch.
pub fn unpremultiply(rgb: [f32; 3], a: f32) -> [f32; 3] {
    if a <= f32::EPSILON {
        return [0.0, 0.0, 0.0];
    }
    [rgb[0] / a, rgb[1] / a, rgb[2] / a]
}

/// Premultiply straight-alpha input on the way in (decoded media, imported
/// assets).
pub fn premultiply(rgb: [f32; 3], a: f32) -> [f32; 3] {
    [rgb[0] * a, rgb[1] * a, rgb[2] * a]
}

/// Assertions that gate a release. Called by `pmgctl --selftest`.
///
/// These are deliberately about the rules most likely to be broken by someone
/// reusing the fill path for the key, rather than about arithmetic nobody
/// doubts.
pub fn selftest() -> Result<(), String> {
    // Narrow-range endpoints land where the spec says. This is what the ramp
    // test pattern with marked 0%/100% points proves on real hardware (S9).
    if encode_key_10bit(0.0, Range::Narrow).0 != 64 {
        return Err("key narrow-range floor is not 64".into());
    }
    if encode_key_10bit(1.0, Range::Narrow).0 != 940 {
        return Err("key narrow-range ceiling is not 940".into());
    }
    if encode_key_10bit(0.0, Range::Full).0 != 0 || encode_key_10bit(1.0, Range::Full).0 != 1023 {
        return Err("key full-range endpoints are wrong".into());
    }

    // The rule. Chroma is neutral at EVERY alpha, in every range. A deviation
    // here means the key went through a colour matrix (ADR 0012 §3).
    for step in 0..=100 {
        let a = step as f32 / 100.0;
        for range in [Range::Narrow, Range::Full] {
            let (_, cb, cr) = encode_key_10bit(a, range);
            if cb != NEUTRAL_CHROMA_10 || cr != NEUTRAL_CHROMA_10 {
                return Err(format!("key chroma not neutral at alpha {a} ({range:?})"));
            }
        }
    }

    // Out-of-range input clamps rather than wrapping.
    if encode_key_10bit(-1.0, Range::Narrow).0 != 64 || encode_key_10bit(2.0, Range::Full).0 != 1023
    {
        return Err("out-of-range alpha did not clamp".into());
    }

    // Premultiply round-trips within tolerance.
    let straight = [0.8_f32, 0.4, 0.2];
    let a = 0.6_f32;
    let back = unpremultiply(premultiply(straight, a), a);
    for i in 0..3 {
        if (back[i] - straight[i]).abs() > 1e-5 {
            return Err("premultiply round-trip lost precision".into());
        }
    }
    // Fully transparent unpremultiplies to black rather than to infinity.
    if unpremultiply([0.0, 0.0, 0.0], 0.0) != [0.0, 0.0, 0.0] {
        return Err("unpremultiply at alpha 0 is not black".into());
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
    fn key_is_monotonic() {
        let mut last = 0;
        for step in 0..=100 {
            let y = encode_key_10bit(step as f32 / 100.0, Range::Narrow).0;
            assert!(y >= last, "key luma must be monotonic in alpha");
            last = y;
        }
    }
}
