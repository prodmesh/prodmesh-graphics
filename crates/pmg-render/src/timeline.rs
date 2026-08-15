//! Evaluating a document's timelines to per-node property values.
//!
//! Pure, like the rest of `pmg-anim`: given a timeline and a `t`, the result
//! depends on nothing else. **No wall clock reaches this code** — frame time is
//! computed from the output's exact rational rate and passed in.

use pmg_scene::doc::{SceneDoc, Timeline};
use std::collections::BTreeMap;

/// Evaluate one named timeline at `t_ms`, returning `node_id/property -> value`.
///
/// Tracks whose target node does not exist are simply absent from the result —
/// the compiler already warned at publish, and a deleted layer must not stop a
/// template rendering (`docs/specs/scene-format.md` §6.2).
pub fn evaluate(timeline: &Timeline, t_ms: f32) -> BTreeMap<String, f32> {
    let mut out = BTreeMap::new();
    for track in &timeline.tracks {
        // Multiple running timelines evaluate in start order and the LAST
        // writer per target wins. Within one timeline, later tracks win, which
        // is the same rule applied consistently (§6.3).
        if let Some(v) = pmg_anim::eval(&track.keys, t_ms) {
            out.insert(track.target.clone(), v);
        }
    }
    out
}

/// Find a timeline by name. `In` and `Out` are reserved and have engine meaning.
pub fn find<'a>(doc: &'a SceneDoc, name: &str) -> Option<&'a Timeline> {
    doc.timelines.iter().find(|t| t.name == name)
}

/// Frame time from an exact rational rate.
///
/// `60000/1001`, never a `59.94` float — so an 800 ms In looks identical at 25p
/// and at 59.94p (`docs/specs/output-config.md` §2).
pub fn frame_time_ms(frame: u64, fps_num: u32, fps_den: u32) -> f32 {
    pmg_anim::frame_time_ms(frame, fps_num, fps_den) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_in_timeline_starts_offscreen_and_lands_at_zero() {
        let doc = pmg_scene::fixture::lower_third();
        let tl = find(&doc, "In").expect("In exists");

        let at_start = evaluate(tl, 0.0);
        let at_end = evaluate(tl, tl.duration_ms);

        assert_eq!(at_start.get("band/transform.translate.x"), Some(&-900.0));
        assert_eq!(at_end.get("band/transform.translate.x"), Some(&0.0));
    }

    #[test]
    fn the_out_timeline_leaves() {
        let doc = pmg_scene::fixture::lower_third();
        let tl = find(&doc, "Out").expect("Out exists");
        assert_eq!(
            evaluate(tl, tl.duration_ms).get("band/transform.translate.x"),
            Some(&-900.0)
        );
    }

    #[test]
    fn evaluation_is_pure() {
        // If this ever fails, something reached for a clock — and golden images
        // and S1's parity comparison both stop meaning anything.
        let doc = pmg_scene::fixture::lower_third();
        let tl = find(&doc, "In").unwrap();
        for step in 0..=16 {
            let t = step as f32 * 50.0;
            assert_eq!(evaluate(tl, t), evaluate(tl, t));
        }
    }

    #[test]
    fn an_800ms_in_is_800ms_at_any_frame_rate() {
        // The reason frame time is rational rather than a float.
        let at_25p = frame_time_ms(20, 25, 1);
        let at_5994 = frame_time_ms(48, 60000, 1001);
        assert!((at_25p - 800.0).abs() < 0.01);
        assert!((at_5994 - 800.0).abs() < 1.0);
    }
}
