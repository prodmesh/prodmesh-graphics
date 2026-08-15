//! Timelines, keyframe tracks, easing, and evaluate-at-t.
//!
//! Spec: `docs/specs/scene-format.md` §6. Decision: ADR 0005.
//!
//! In the **pure set**: no I/O, no platform, and — the invariant that matters
//! here — **no wall clock**. Given a timeline and a `t`, the result depends on
//! nothing else. That is what makes an 800 ms `In` look identical at 25p and at
//! 59.94p, and what makes golden-image tests meaningful.

#![forbid(unsafe_code)]

/// Frame time from an exact rational frame rate.
///
/// `60000/1001`, never a `59.94` float. A float frame rate accumulates error
/// over a service and there is no reason to accept it
/// (`docs/specs/output-config.md` §2).
pub fn frame_time_ms(frames: u64, fps_num: u32, fps_den: u32) -> f64 {
    (frames as f64) * 1000.0 * (fps_den as f64) / (fps_num as f64)
}

/// Interpolation between two keys. Applied from the *preceding* key.
#[derive(Debug, Clone, Copy, PartialEq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ease {
    /// The default when a key omits `ease`, so an unadorned track is linear
    /// rather than accidentally eased.
    #[default]
    Linear,
    /// CSS-style cubic Bézier control points, `[x1, y1, x2, y2]`.
    Cubic([f32; 4]),
    Steps(u32),
}

impl Ease {
    /// Map normalised progress `p` in [0,1] through the easing curve.
    pub fn apply(&self, p: f32) -> f32 {
        let p = p.clamp(0.0, 1.0);
        match self {
            Ease::Linear => p,
            Ease::Steps(n) => {
                let n = (*n).max(1) as f32;
                (p * n).floor() / n
            }
            Ease::Cubic([x1, y1, x2, y2]) => cubic_bezier(p, *x1, *y1, *x2, *y2),
        }
    }
}

/// Solve a CSS-style cubic Bézier for y given x, by bisection.
///
/// Bisection rather than Newton-Raphson deliberately: it is branch-predictable,
/// has no divergence case, and terminates in a fixed number of iterations. This
/// runs on the render thread, where a bounded worst case is worth more than a
/// faster average.
fn cubic_bezier(x: f32, x1: f32, y1: f32, x2: f32, y2: f32) -> f32 {
    fn curve(t: f32, a: f32, b: f32) -> f32 {
        let u = 1.0 - t;
        3.0 * u * u * t * a + 3.0 * u * t * t * b + t * t * t
    }
    let (mut lo, mut hi) = (0.0_f32, 1.0_f32);
    let mut t = x;
    for _ in 0..24 {
        let cx = curve(t, x1, x2);
        if (cx - x).abs() < 1e-5 {
            break;
        }
        if cx < x {
            lo = t;
        } else {
            hi = t;
        }
        t = 0.5 * (lo + hi);
    }
    curve(t, y1, y2)
}

/// One keyframe on a track.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct Key {
    /// Milliseconds from the timeline's start.
    pub t: f32,
    pub v: f32,
    #[serde(default)]
    pub ease: Ease,
}

/// Evaluate a sorted key list at time `t_ms`.
///
/// Before the first key, hold the first value; after the last, hold the last
/// (`docs/specs/scene-format.md` §6.3).
pub fn eval(keys: &[Key], t_ms: f32) -> Option<f32> {
    let first = keys.first()?;
    let last = keys.last()?;
    if t_ms <= first.t {
        return Some(first.v);
    }
    if t_ms >= last.t {
        return Some(last.v);
    }
    let i = keys.partition_point(|k| k.t <= t_ms) - 1;
    let (a, b) = (&keys[i], &keys[i + 1]);
    let span = b.t - a.t;
    if span <= f32::EPSILON {
        return Some(b.v);
    }
    let p = a.ease.apply((t_ms - a.t) / span);
    Some(a.v + (b.v - a.v) * p)
}

/// Assertions that gate a release. Called by `pmgctl --selftest`.
pub fn selftest() -> Result<(), String> {
    // Exact rational frame time. 60000/1001 for 48 frames is ~800.8 ms; the
    // point is that it is computed from the ratio, not from 59.94.
    let t = frame_time_ms(48, 60000, 1001);
    if (t - 800.8).abs() > 0.01 {
        return Err(format!("59.94p frame time wrong: {t}"));
    }
    if (frame_time_ms(25, 25, 1) - 1000.0).abs() > 1e-9 {
        return Err("25p frame time wrong".into());
    }

    let keys = [
        Key {
            t: 0.0,
            v: -400.0,
            ease: Ease::Cubic([0.2, 0.0, 0.0, 1.0]),
        },
        Key {
            t: 800.0,
            v: 0.0,
            ease: Ease::Linear,
        },
    ];

    // Endpoints are exact, and holding outside the range is exact.
    if eval(&keys, 0.0) != Some(-400.0) || eval(&keys, 800.0) != Some(0.0) {
        return Err("timeline endpoints are not exact".into());
    }
    if eval(&keys, -100.0) != Some(-400.0) || eval(&keys, 5000.0) != Some(0.0) {
        return Err("timeline does not hold outside its range".into());
    }

    // Monotonic, and purely a function of t: evaluating twice gives the same
    // answer. If this ever fails, something reached for a clock.
    let mut last = f32::NEG_INFINITY;
    for step in 0..=80 {
        let t = step as f32 * 10.0;
        let v = eval(&keys, t).ok_or("eval returned None")?;
        if eval(&keys, t) != Some(v) {
            return Err("timeline evaluation is not pure".into());
        }
        if v < last - 1e-4 {
            return Err(format!("ease-out curve is not monotonic at t={t}"));
        }
        last = v;
    }

    if eval(&[], 0.0).is_some() {
        return Err("empty track should evaluate to None".into());
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
    fn linear_ease_is_identity() {
        for step in 0..=10 {
            let p = step as f32 / 10.0;
            assert!((Ease::Linear.apply(p) - p).abs() < 1e-6);
        }
    }

    #[test]
    fn same_duration_regardless_of_frame_rate() {
        // An 800 ms In must land at the same progress at 25p and 59.94p.
        let at_25p = frame_time_ms(20, 25, 1); // 800.0 ms
        let at_5994 = frame_time_ms(48, 60000, 1001); // ~800.8 ms
        assert!((at_25p - 800.0).abs() < 1e-6);
        assert!((at_5994 - 800.0).abs() < 1.0);
    }
}
