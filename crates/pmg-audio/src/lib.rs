//! Mixing, resampling, and layer → output channel-pair mapping.
//!
//! Not in the pure set. Spec: `docs/specs/output-config.md` §7.
//!
//! # The video output is the clock
//!
//! Audio does not have a rate of its own here. It has a number of samples that
//! must accompany each video frame, and that number is decided by the video
//! clock (`docs/RISKS.md` R5). Everything in this module follows from that.
//!
//! # Why the sample count per frame is not a constant
//!
//! At 48 kHz against 60000/1001 fps, a frame carries **800.8 samples**. There is
//! no such thing as four fifths of a sample, so the count has to alternate on an
//! exact cadence — 801, 801, 801, 801, 800 — that averages out precisely.
//!
//! Getting this wrong is the classic broadcast audio bug and it is invisible for
//! the first hour. Rounding 800.8 down loses 0.8 samples a frame: 48 samples a
//! second, a millisecond of lip-sync error every second, **three and a half
//! seconds of drift over a two-hour service.** Nobody notices at the top of the
//! hour and everybody notices by the end.
//!
//! So the count is derived the same way the video clock derives media frames
//! (`pmg_media::video::MediaClock`): an exact integer expression evaluated
//! against the absolute frame number, never an accumulator that carries a
//! remainder and never a float. Two expressions of the same discipline, because
//! this is the one place where a rounding choice compounds instead of cancelling.

/// Samples per second. 48 kHz is the only rate broadcast plant uses, and
/// accepting others would mean resampling on the frame path to no purpose.
pub const SAMPLE_RATE: u32 = 48_000;

#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("output has {have} channels, but a pair at index {pair} needs {need}")]
    NoSuchPair {
        pair: usize,
        need: usize,
        have: usize,
    },
    #[error("{0} Hz is not supported; broadcast audio is 48 kHz")]
    UnsupportedRate(u32),
}

/// How many audio samples ride with each video frame.
///
/// Answers against the **absolute** frame number rather than by accumulating,
/// so an output that has been up for six hours is as exact as one that just
/// started, and a restart cannot inherit a stale remainder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioClock {
    fps_num: u32,
    fps_den: u32,
    rate: u32,
}

impl AudioClock {
    pub fn new(fps: (u32, u32), rate: u32) -> Result<Self, AudioError> {
        if rate != SAMPLE_RATE {
            return Err(AudioError::UnsupportedRate(rate));
        }
        Ok(Self {
            fps_num: fps.0.max(1),
            fps_den: fps.1.max(1),
            rate,
        })
    }

    /// Total samples that should have been sent *before* frame `n`.
    ///
    /// `floor(n * rate * fps_den / fps_num)`, in `u128` so a long service cannot
    /// overflow the intermediate: six hours at 59.94 is already 1.3 million
    /// frames times 48000 times 1001.
    pub fn samples_before(&self, n: u64) -> u64 {
        let num = n as u128 * self.rate as u128 * self.fps_den as u128;
        (num / self.fps_num as u128) as u64
    }

    /// Samples belonging to frame `n`.
    ///
    /// The difference of two exact totals, which is what makes the cadence fall
    /// out on its own rather than needing to be written down: at 59.94 this
    /// returns 801, 801, 801, 801, 800, forever, with no state.
    pub fn samples_for(&self, n: u64) -> u32 {
        (self.samples_before(n + 1) - self.samples_before(n)) as u32
    }

    /// The nominal, fractional count — for reporting, never for scheduling.
    pub fn samples_per_frame(&self) -> f64 {
        self.rate as f64 * self.fps_den as f64 / self.fps_num as f64
    }

    /// How long the cadence takes to repeat, in frames.
    ///
    /// Worth surfacing because it is the natural period for any buffering: at
    /// 59.94 it is 5 frames, at 29.97 it is 5, and at 25 it is 1.
    pub fn cadence_frames(&self) -> u64 {
        let g = gcd(self.rate as u64 * self.fps_den as u64, self.fps_num as u64);
        self.fps_num as u64 / g
    }
}

fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 {
        a
    } else {
        gcd(b, a % b)
    }
}

/// Which layers' audio lands on which output channel pair.
///
/// Part of **output** configuration rather than a template property: the same
/// template played to a lower-third key and to an in-room wall may need its
/// audio in different places, or in one case nowhere at all
/// (`docs/specs/output-config.md` §7).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PairMap {
    /// Index i is output pair i; the strings are layer names, mixed together.
    pub pairs: Vec<Vec<String>>,
}

impl PairMap {
    pub fn channels(&self) -> usize {
        self.pairs.len() * 2
    }

    /// Which pair a layer's audio belongs to, if any.
    ///
    /// `None` means **this layer is silent on this output**, which is a normal
    /// configuration and not a fault: a lower third over a key feed usually has
    /// nothing to say.
    pub fn pair_of(&self, layer: &str) -> Option<usize> {
        self.pairs
            .iter()
            .position(|ls| ls.iter().any(|l| l == layer))
    }
}

/// Interleaved stereo samples for one video frame, on one pair.
pub type Pair = Vec<i32>;

/// Mix layer audio into an interleaved multi-channel buffer for one frame.
///
/// `sources` is `(layer, interleaved stereo)`. Everything is 32-bit signed,
/// which is what DeckLink wants and what leaves headroom for summing several
/// layers before the clip at the end.
///
/// Samples are **summed and then clipped**, not averaged. Averaging would duck
/// every layer whenever a second one appeared, which is a mix decision this code
/// has no business making; clipping is at least audible and attributable.
pub fn mix_frame(
    map: &PairMap,
    sources: &[(String, &[i32])],
    samples: usize,
) -> Result<Vec<i32>, AudioError> {
    let channels = map.channels();
    let mut out = vec![0i64; samples * channels];

    for (layer, pcm) in sources {
        let Some(pair) = map.pair_of(layer) else {
            continue;
        };
        if pair * 2 + 1 >= channels {
            return Err(AudioError::NoSuchPair {
                pair,
                need: pair * 2 + 2,
                have: channels,
            });
        }
        // A source short of this frame's sample count contributes what it has
        // and no more. Silence for the remainder beats reaching past the end,
        // and it is what an underrunning decoder should sound like.
        let n = samples.min(pcm.len() / 2);
        for s in 0..n {
            out[s * channels + pair * 2] += pcm[s * 2] as i64;
            out[s * channels + pair * 2 + 1] += pcm[s * 2 + 1] as i64;
        }
    }

    Ok(out
        .into_iter()
        .map(|v| v.clamp(i32::MIN as i64, i32::MAX as i64) as i32)
        .collect())
}

pub fn selftest() -> Result<(), String> {
    // Two hours at 59.94 must carry exactly the right number of samples. A
    // per-frame rounding error of 0.8 samples is inaudible for an hour and is
    // three and a half seconds of lip sync by the end of a service — which is
    // why this is in the selftest and not only in a unit test.
    let c = AudioClock::new((60000, 1001), SAMPLE_RATE).map_err(|e| e.to_string())?;
    let two_hours = 431_568u64;
    let want = (two_hours as u128 * 48_000 * 1001 / 60_000) as u64;
    let got = c.samples_before(two_hours);
    if got != want {
        return Err(format!(
            "two hours at 59.94 should carry {want} samples, got {got} \
             (a drift of {} ms)",
            (want as i64 - got as i64) * 1000 / 48_000
        ));
    }
    // And the cadence itself must be exactly the classic one.
    let five: Vec<u32> = (0..5).map(|n| c.samples_for(n)).collect();
    if five.iter().map(|v| *v as u64).sum::<u64>() != 4004 {
        return Err(format!(
            "59.94 cadence should sum to 4004 over 5 frames: {five:?}"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn twenty_five_is_a_whole_number_of_samples() {
        let c = AudioClock::new((25, 1), SAMPLE_RATE).unwrap();
        assert_eq!(c.samples_for(0), 1920);
        assert_eq!(c.samples_for(12345), 1920);
        assert_eq!(c.cadence_frames(), 1, "no cadence needed at 25p");
    }

    /// The one everybody gets wrong. 800.8 samples a frame cannot be 800.
    #[test]
    fn fifty_nine_ninety_four_alternates_on_a_five_frame_cadence() {
        let c = AudioClock::new((60000, 1001), SAMPLE_RATE).unwrap();
        assert!((c.samples_per_frame() - 800.8).abs() < 1e-9);
        assert_eq!(c.cadence_frames(), 5);
        let five: Vec<u32> = (0..5).map(|n| c.samples_for(n)).collect();
        assert_eq!(
            five.iter().sum::<u32>(),
            4004,
            "five frames carry 4004 samples"
        );
        // Every frame is one of the two neighbouring integers — never 800.8
        // rounded to something convenient.
        assert!(five.iter().all(|&s| s == 800 || s == 801), "{five:?}");
    }

    #[test]
    fn twenty_nine_ninety_seven_is_the_ntsc_cadence() {
        let c = AudioClock::new((30000, 1001), SAMPLE_RATE).unwrap();
        let five: Vec<u32> = (0..5).map(|n| c.samples_for(n)).collect();
        assert_eq!(five.iter().sum::<u32>(), 8008);
        assert!(five.iter().all(|&s| s == 1601 || s == 1602), "{five:?}");
    }

    /// What the whole module exists to prevent.
    ///
    /// Truncating to 800 samples a frame loses 48 samples a second. Over two
    /// hours that is 345,600 samples — **7.2 seconds** — and the point of the
    /// test is that the exact path shows zero over the same span.
    #[test]
    fn two_hours_does_not_drift_a_single_sample() {
        let c = AudioClock::new((60000, 1001), SAMPLE_RATE).unwrap();
        let two_hours = 431_568u64;

        let exact = c.samples_before(two_hours);
        let summed: u64 = (0..two_hours).map(|n| c.samples_for(n) as u64).sum();
        assert_eq!(
            exact, summed,
            "per-frame counts must sum to the exact total"
        );

        let naive = two_hours * 800;
        let lost_ms = (exact - naive) * 1000 / SAMPLE_RATE as u64;
        assert!(
            lost_ms > 3000,
            "truncating should lose seconds, not milliseconds — got {lost_ms} ms"
        );
    }

    #[test]
    fn samples_before_is_monotonic_and_starts_at_zero() {
        let c = AudioClock::new((60000, 1001), SAMPLE_RATE).unwrap();
        assert_eq!(c.samples_before(0), 0);
        let mut last = 0;
        for n in 0..1000 {
            let s = c.samples_before(n);
            assert!(s >= last);
            last = s;
        }
    }

    #[test]
    fn a_rate_that_is_not_48k_is_refused() {
        assert!(matches!(
            AudioClock::new((25, 1), 44_100),
            Err(AudioError::UnsupportedRate(44_100))
        ));
    }

    fn map(pairs: &[&[&str]]) -> PairMap {
        PairMap {
            pairs: pairs
                .iter()
                .map(|p| p.iter().map(|s| s.to_string()).collect())
                .collect(),
        }
    }

    #[test]
    fn a_layer_with_no_pair_is_silent_rather_than_an_error() {
        let m = map(&[&["Fullscreen"]]);
        assert_eq!(m.pair_of("Fullscreen"), Some(0));
        assert_eq!(m.pair_of("Lower Third"), None);

        let pcm = vec![100i32; 8];
        let out = mix_frame(&m, &[("Lower Third".into(), &pcm)], 4).unwrap();
        assert!(
            out.iter().all(|&s| s == 0),
            "an unmapped layer contributes nothing"
        );
    }

    #[test]
    fn layers_on_one_pair_sum_and_layers_on_different_pairs_do_not() {
        let m = map(&[&["A"], &["B"]]);
        let a = vec![10i32; 8];
        let b = vec![3i32; 8];
        let out = mix_frame(&m, &[("A".into(), &a), ("B".into(), &b)], 4).unwrap();
        assert_eq!(out.len(), 4 * 4, "4 samples across 2 pairs");
        assert_eq!(out[0], 10, "pair 0 left");
        assert_eq!(out[2], 3, "pair 1 left is untouched by A");

        let m2 = map(&[&["A", "B"]]);
        let out2 = mix_frame(&m2, &[("A".into(), &a), ("B".into(), &b)], 4).unwrap();
        assert_eq!(out2[0], 13, "same pair sums, it does not average");
    }

    #[test]
    fn a_short_source_contributes_what_it_has() {
        let m = map(&[&["A"]]);
        // Two samples of audio where the frame wants four.
        let a = vec![7i32; 4];
        let out = mix_frame(&m, &[("A".into(), &a)], 4).unwrap();
        assert_eq!(&out[..4], &[7, 7, 7, 7]);
        assert_eq!(&out[4..], &[0, 0, 0, 0], "the rest is silence, not garbage");
    }

    #[test]
    fn summing_clips_rather_than_wrapping() {
        let m = map(&[&["A", "B"]]);
        let loud = vec![i32::MAX; 2];
        let out = mix_frame(&m, &[("A".into(), &loud), ("B".into(), &loud)], 1).unwrap();
        assert_eq!(out[0], i32::MAX, "clipped, not wrapped to a huge negative");
    }
}
