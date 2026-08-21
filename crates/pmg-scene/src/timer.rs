//! Timers — the P1 dynamic input (ADR 0022).
//!
//! A timer is a **pure function of the clock**:
//!
//! ```text
//! format_at(spec, now_ms, taken_at_ms) -> String
//! ```
//!
//! No I/O, no state, no failure mode. Same inputs, same string, on a booth Mac
//! and in a browser — which is what lets the node compute it from its own clock
//! (so a countdown keeps counting when the server dies), lets the designer
//! preview it, and lets the golden corpus hold a timer figure at a **pinned**
//! instant like any other.
//!
//! The value never travels over the link. The Item carries this declaration and
//! the node evaluates it; ADR 0022 has the reasoning and the alternative that
//! loses.

use serde::{Deserialize, Serialize};

/// What a timer counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimerKind {
    /// What time is it — the NODE's local time (ADR 0022's consequence about
    /// clock skew).
    Clock,
    /// How long until `at`. Floors at zero and stays there: a countdown that
    /// reads `0:00` is telling the truth, and one that blanks takes the graphic
    /// off the wall at the moment everyone is looking at it.
    Countdown,
    /// How long since `at`.
    Countup,
    /// How long since this Item was taken. `at` is ignored.
    Elapsed,
}

/// How it reads.
///
/// **Enumerated, not a format string.** A strftime-shaped mini language is a
/// parser, and a parser is a thing that fails at air time; this is one match
/// statement that cannot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimerFormat {
    /// `01:04:32`
    HhMmSs,
    /// `1:04:32`
    HMmSs,
    /// `04:32`
    MmSs,
    /// `4:32`
    MSs,
    /// `10:05`
    HhMm,
    /// `10:05`, no leading zero on the hour
    HMm,
    /// `10:05 AM`
    HMmAmPm,
}

/// The declaration an Item carries.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TimerSpec {
    pub kind: TimerKind,
    /// The instant this counts to or from, in **UTC milliseconds** — so the
    /// arithmetic needs no timezone at all. Absent for `clock` and `elapsed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<i64>,
    pub format: TimerFormat,
}

impl TimerSpec {
    /// A countdown to an instant, in the format most countdowns want.
    pub fn countdown_to(at_ms: i64) -> Self {
        TimerSpec {
            kind: TimerKind::Countdown,
            at: Some(at_ms),
            format: TimerFormat::MSs,
        }
    }
}

/// Milliseconds in a day, and the pieces of one.
const MS_SEC: i64 = 1000;
const MS_MIN: i64 = 60 * MS_SEC;
const MS_HOUR: i64 = 60 * MS_MIN;
const MS_DAY: i64 = 24 * MS_HOUR;

/// Split a non-negative duration into whole hours, minutes, seconds.
///
/// **Truncating, not rounding.** A countdown that rounds shows `0:01` for the
/// last one and a half seconds and then jumps to `0:00` early; truncation means
/// the number on screen is the number of whole units actually left, which is
/// what a person reading it assumes.
fn hms(ms: i64) -> (i64, i64, i64) {
    let ms = ms.max(0);
    (ms / MS_HOUR, (ms / MS_MIN) % 60, (ms / MS_SEC) % 60)
}

/// Render a duration.
fn duration(ms: i64, f: TimerFormat) -> String {
    let (h, m, s) = hms(ms);
    match f {
        TimerFormat::HhMmSs => format!("{h:02}:{m:02}:{s:02}"),
        TimerFormat::HMmSs => format!("{h}:{m:02}:{s:02}"),
        // A duration past an hour still has to READ right in a minutes format,
        // so the minutes carry the hours rather than wrapping silently: 90
        // minutes is "90:00", not "30:00". Wrapping would be a lie that looks
        // like a working clock.
        TimerFormat::MmSs => format!("{:02}:{s:02}", h.saturating_mul(60) + m),
        TimerFormat::MSs => format!("{}:{s:02}", h.saturating_mul(60) + m),
        TimerFormat::HhMm => format!("{h:02}:{m:02}"),
        TimerFormat::HMm => format!("{h}:{m:02}"),
        TimerFormat::HMmAmPm => format!("{h}:{m:02} AM"),
    }
}

/// Render a wall-clock instant, given milliseconds since local midnight.
fn wall(ms_into_day: i64, f: TimerFormat) -> String {
    let ms = ms_into_day.rem_euclid(MS_DAY);
    let (h24, m, s) = (ms / MS_HOUR, (ms / MS_MIN) % 60, (ms / MS_SEC) % 60);
    match f {
        TimerFormat::HhMmSs => format!("{h24:02}:{m:02}:{s:02}"),
        TimerFormat::HMmSs => format!("{h24}:{m:02}:{s:02}"),
        TimerFormat::MmSs => format!("{m:02}:{s:02}"),
        TimerFormat::MSs => format!("{m}:{s:02}"),
        TimerFormat::HhMm => format!("{h24:02}:{m:02}"),
        TimerFormat::HMm => format!("{h24}:{m:02}"),
        TimerFormat::HMmAmPm => {
            // 0 reads as 12, and so does 12 — the two ends people get wrong.
            let h12 = match h24 % 12 {
                0 => 12,
                h => h,
            };
            let ampm = if h24 < 12 { "AM" } else { "PM" };
            format!("{h12}:{m:02} {ampm}")
        }
    }
}

/// Evaluate a timer.
///
/// - `now_ms` — UTC milliseconds.
/// - `local_offset_ms` — the node's offset from UTC, for `clock` only. Passed
///   in rather than read here, because this crate is in the pure set and asking
///   the operating system what time zone it is in is I/O.
/// - `taken_at_ms` — when the Item went on air, for `elapsed`.
///
/// Total: every input produces a string, including a `countdown` with no `at`
/// (which counts to now, so it reads zero — a declaration missing its instant
/// shows a stopped clock rather than taking the graphic down).
pub fn format_at(spec: &TimerSpec, now_ms: i64, local_offset_ms: i64, taken_at_ms: i64) -> String {
    // Saturating throughout. An instant far enough from now to overflow an
    // i64 is nonsense either way, and the choice is between a clamped number
    // and a panic on the thread that draws frames.
    match spec.kind {
        TimerKind::Clock => wall(now_ms.saturating_add(local_offset_ms), spec.format),
        TimerKind::Countdown => duration(
            spec.at.unwrap_or(now_ms).saturating_sub(now_ms),
            spec.format,
        ),
        TimerKind::Countup => duration(
            now_ms.saturating_sub(spec.at.unwrap_or(now_ms)),
            spec.format,
        ),
        TimerKind::Elapsed => duration(now_ms.saturating_sub(taken_at_ms), spec.format),
    }
}

/// How long until this timer's rendered string changes, in milliseconds.
///
/// The node uses this to know it has nothing to do. A seconds format changes on
/// the next second boundary; a minutes format on the next minute. Returning the
/// exact remainder rather than "one second" is what keeps a clock from drifting
/// a frame later every minute.
///
/// `None` means it never changes on its own — a countdown that has reached zero
/// has nothing left to say.
pub fn ms_until_change(spec: &TimerSpec, now_ms: i64, taken_at_ms: i64) -> Option<i64> {
    let step = match spec.format {
        TimerFormat::HhMm | TimerFormat::HMm | TimerFormat::HMmAmPm => MS_MIN,
        _ => MS_SEC,
    };
    // Where the counted quantity sits inside its current step.
    let position = match spec.kind {
        TimerKind::Clock => now_ms,
        TimerKind::Countup => now_ms.saturating_sub(spec.at.unwrap_or(now_ms)),
        TimerKind::Elapsed => now_ms.saturating_sub(taken_at_ms),
        TimerKind::Countdown => {
            let left = spec.at.unwrap_or(now_ms).saturating_sub(now_ms);
            if left <= 0 {
                // Floored, and staying there.
                return None;
            }
            // Counting DOWN truncates, so the display changes when the
            // remaining time crosses the step boundary BELOW it, which is a
            // different instant from the one a count-up would change at.
            return Some((left.saturating_sub(1).rem_euclid(step)) + 1);
        }
    };
    Some(step - position.rem_euclid(step))
}

#[cfg(test)]
mod tests {
    use super::*;

    const T10: i64 = 1_755_691_200_000; // an arbitrary instant, used as "10:00"

    fn spec(kind: TimerKind, at: Option<i64>, format: TimerFormat) -> TimerSpec {
        TimerSpec { kind, at, format }
    }

    #[test]
    fn a_countdown_counts_down_and_floors_at_zero() {
        let s = spec(TimerKind::Countdown, Some(T10), TimerFormat::MSs);
        assert_eq!(format_at(&s, T10 - 272_000, 0, 0), "4:32");
        assert_eq!(format_at(&s, T10 - 1_000, 0, 0), "0:01");
        assert_eq!(format_at(&s, T10, 0, 0), "0:00");
        // Past the target it stays at zero rather than going negative or
        // blanking — the graphic is on the wall at exactly this moment.
        assert_eq!(format_at(&s, T10 + 60_000, 0, 0), "0:00");
    }

    #[test]
    fn a_countdown_truncates_so_the_number_is_whole_units_left() {
        let s = spec(TimerKind::Countdown, Some(T10), TimerFormat::MSs);
        // 1.9 s left is "1 second left", not "2".
        assert_eq!(format_at(&s, T10 - 1_900, 0, 0), "0:01");
        assert_eq!(format_at(&s, T10 - 999, 0, 0), "0:00");
    }

    #[test]
    fn minutes_formats_carry_the_hours_rather_than_wrapping() {
        let s = spec(TimerKind::Countdown, Some(T10), TimerFormat::MmSs);
        // 90 minutes is 90:00. Wrapping to 30:00 would be a lie that looks
        // exactly like a working clock.
        assert_eq!(format_at(&s, T10 - 90 * 60_000, 0, 0), "90:00");
    }

    #[test]
    fn elapsed_counts_from_the_take() {
        let s = spec(TimerKind::Elapsed, None, TimerFormat::MmSs);
        assert_eq!(format_at(&s, T10 + 65_000, 0, T10), "01:05");
        assert_eq!(format_at(&s, T10, 0, T10), "00:00");
    }

    #[test]
    fn the_clock_reads_the_node_s_local_time() {
        let s = spec(TimerKind::Clock, None, TimerFormat::HhMm);
        // Midnight UTC plus ten hours of offset is 10:00 locally.
        let midnight = 1_755_648_000_000; // a UTC midnight
        assert_eq!(format_at(&s, midnight, 10 * MS_HOUR, 0), "10:00");
        assert_eq!(format_at(&s, midnight, -5 * MS_HOUR, 0), "19:00");
    }

    #[test]
    fn twelve_hour_gets_both_ends_right() {
        let s = spec(TimerKind::Clock, None, TimerFormat::HMmAmPm);
        let midnight = 1_755_648_000_000;
        assert_eq!(format_at(&s, midnight, 0, 0), "12:00 AM");
        assert_eq!(format_at(&s, midnight, 12 * MS_HOUR, 0), "12:00 PM");
        assert_eq!(format_at(&s, midnight, 13 * MS_HOUR, 0), "1:00 PM");
        assert_eq!(format_at(&s, midnight, 23 * MS_HOUR, 0), "11:00 PM");
    }

    /// Every input produces a string. There is no error path, because there is
    /// no error path available at air time.
    #[test]
    fn it_is_total() {
        for kind in [
            TimerKind::Clock,
            TimerKind::Countdown,
            TimerKind::Countup,
            TimerKind::Elapsed,
        ] {
            for at in [None, Some(0), Some(i64::MIN), Some(i64::MAX)] {
                for format in [
                    TimerFormat::HhMmSs,
                    TimerFormat::HMmSs,
                    TimerFormat::MmSs,
                    TimerFormat::MSs,
                    TimerFormat::HhMm,
                    TimerFormat::HMm,
                    TimerFormat::HMmAmPm,
                ] {
                    for now in [0, T10, i64::MIN / 2, i64::MAX / 2] {
                        let s = format_at(&spec(kind, at, format), now, 0, T10);
                        assert!(!s.is_empty());
                    }
                }
            }
        }
    }

    /// The node asks "how long may I do nothing?" — and the answer has to land
    /// exactly on the boundary, or a clock drifts a frame later every minute.
    #[test]
    fn the_next_change_lands_on_the_boundary() {
        let clock = spec(TimerKind::Clock, None, TimerFormat::HhMmSs);
        assert_eq!(ms_until_change(&clock, 1_000, 0), Some(1_000));
        assert_eq!(ms_until_change(&clock, 1_400, 0), Some(600));
        assert_eq!(ms_until_change(&clock, 1_999, 0), Some(1));

        let per_minute = spec(TimerKind::Clock, None, TimerFormat::HhMm);
        assert_eq!(
            ms_until_change(&per_minute, MS_MIN + 1_500, 0),
            Some(58_500)
        );
    }

    #[test]
    fn a_countdown_asks_again_at_its_own_boundary() {
        let s = spec(TimerKind::Countdown, Some(T10), TimerFormat::MSs);
        // 4.5 s left reads "0:04" and becomes "0:03" in 500 ms.
        assert_eq!(format_at(&s, T10 - 4_500, 0, 0), "0:04");
        assert_eq!(ms_until_change(&s, T10 - 4_500, 0), Some(500));
        // Exactly on a second: the display changes a full second later.
        assert_eq!(ms_until_change(&s, T10 - 4_000, 0), Some(1_000));
        // Finished: nothing left to say.
        assert_eq!(ms_until_change(&s, T10, 0), None);
        assert_eq!(ms_until_change(&s, T10 + 5_000, 0), None);
    }

    /// Walking a countdown a frame at a time must produce every second exactly
    /// once, in order — the property a jittering clock breaks.
    #[test]
    fn a_frame_walk_shows_every_second_once() {
        let s = spec(TimerKind::Countdown, Some(T10), TimerFormat::MSs);
        let mut seen: Vec<String> = Vec::new();
        // 59.94 fps for ten seconds, starting a bit off a boundary.
        let start = T10 - 10_000 - 137;
        for f in 0..600 {
            let now = start + (f * 1000 * 1001) / 60000;
            let s = format_at(&s, now, 0, 0);
            if seen.last().map(|p| p != &s).unwrap_or(true) {
                seen.push(s);
            }
        }
        let expected: Vec<String> = (0..=10).rev().map(|n| format!("0:{n:02}")).collect();
        assert_eq!(seen, expected, "every second, once, in order");
    }
}
