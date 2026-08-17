//! The cue rules, which are the part of the node that must never drift.
//!
//! These test the decisions that happen *before* anything expensive: dedupe,
//! deadlines, and the reserved `at` field. They deliberately use a Library that
//! owns nothing, because the point is that these answers do not depend on
//! whether the content is there — a duplicate cue must dedupe whether or not the
//! item exists, and a late take must be refused before it is armed.

use pmg_node::engine::{LayerEngine, Library, Phase};
use pmg_proto::*;
use std::collections::BTreeMap;

/// A library with no content at all.
struct Empty;

impl Library for Empty {
    fn template(&self, _rev: &str) -> Option<&pmg_scene::doc::SceneDoc> {
        None
    }
    fn item(&self, _id: &str) -> Option<(&str, &str, &BTreeMap<String, DataValue>)> {
        None
    }
    fn text_engine(&self) -> anyhow::Result<pmg_text::TextEngine> {
        Ok(pmg_text::TextEngine::new(pmg_text::FontDb::new()))
    }
    fn image(&self, _key: &str) -> Option<(u32, u32, std::sync::Arc<Vec<u8>>)> {
        None
    }
}

fn engine() -> LayerEngine {
    let mut e = LayerEngine::new();
    e.configure(
        &[OutputAssignment {
            id: "out-0".into(),
            mode: "1920x1080".into(),
            color: Default::default(),
            layers: vec!["Lower Third".into(), "Bug".into()],
        }],
        (60000, 1001),
    );
    e
}

fn cue(seq: u64, id: &str, op: CueOp) -> Cue {
    Cue {
        cue_id: id.into(),
        seq,
        epoch: 1,
        deadline_ms: Some(DEFAULT_TAKE_DEADLINE_MS),
        at: None,
        op,
    }
}

fn clear_all() -> CueOp {
    CueOp::ClearAll {
        output: "out-0".into(),
    }
}

#[test]
fn a_scheduled_take_is_refused_rather_than_fired_now() {
    let mut e = engine();
    let mut c = cue(1, "a", clear_all());
    c.at = Some(CueAt::Frame(1_841_944));

    let ack = e.apply(&c, &Empty, 0, false);
    // Silently ignoring `at` would fire it immediately, and nobody would find
    // out. v1 does not support scheduling, so it says so (pmg-link.md §5.5).
    assert_eq!(ack.result, CueResult::Rejected);
    assert_eq!(ack.reason, Some(RejectReason::Unsupported));
    assert_eq!(e.last_applied_seq, 0, "a rejected cue does not advance seq");
}

#[test]
fn the_same_cue_id_twice_is_one_take() {
    let mut e = engine();
    assert_eq!(
        e.apply(&cue(1, "press-1", clear_all()), &Empty, 0, false)
            .result,
        CueResult::Applied
    );
    // Same human action arriving twice — a double-click, a bouncing GPI
    // contact, an HTTP retry after a timeout. All the same press.
    let again = e.apply(&cue(2, "press-1", clear_all()), &Empty, 0, false);
    assert_eq!(again.result, CueResult::Deduped);
}

#[test]
fn a_replayed_seq_after_a_reconnect_is_deduped() {
    let mut e = engine();
    e.apply(&cue(5, "a", clear_all()), &Empty, 0, false);
    // The server replays from its ring on reconnect. Reapplying would clear a
    // layer the operator has since refilled.
    let replay = e.apply(&cue(5, "b", clear_all()), &Empty, 0, false);
    assert_eq!(replay.result, CueResult::Deduped);
    let older = e.apply(&cue(3, "c", clear_all()), &Empty, 0, false);
    assert_eq!(older.result, CueResult::Deduped);
}

#[test]
fn a_late_take_is_rejected_but_a_late_arm_is_not() {
    let mut e = engine();

    let late = e.apply(&cue(1, "late", clear_all()), &Empty, 0, true);
    assert_eq!(late.result, CueResult::Rejected);
    assert_eq!(late.reason, Some(RejectReason::DeadlinePassed));

    // Arming is invisible, so being late costs nothing. It fails here only
    // because the item does not exist — which is the point: lateness was not
    // the reason.
    let arm = e.apply(
        &cue(
            2,
            "arm",
            CueOp::Arm {
                item: "nope".into(),
                output: "out-0".into(),
                layer: "Lower Third".into(),
            },
        ),
        &Empty,
        0,
        true,
    );
    assert_eq!(arm.reason, Some(RejectReason::NoSuchItem));
}

#[test]
fn a_new_epoch_clears_the_idempotency_keys() {
    let mut e = engine();
    e.set_epoch(1);
    e.apply(&cue(1, "same", clear_all()), &Empty, 0, false);

    // The server restarted: seq restarts from 1 and it may reuse ids it has
    // already used. Keeping the old keys would silently drop the first real cue
    // of the new epoch.
    e.set_epoch(2);
    let after = e.apply(&cue(1, "same", clear_all()), &Empty, 0, false);
    assert_eq!(after.result, CueResult::Applied);
}

#[test]
fn layers_are_addressed_by_name() {
    let mut e = engine();
    let ack = e.apply(
        &cue(
            1,
            "a",
            CueOp::ClearLayer {
                output: "out-0".into(),
                layer: "Middle Third".into(),
            },
        ),
        &Empty,
        0,
        false,
    );
    // A misnamed layer is refused by name rather than resolved to an index and
    // silently applied to whatever happens to be at that position.
    assert_eq!(ack.reason, Some(RejectReason::NoSuchLayer));
}

#[test]
fn taking_out_an_empty_layer_is_success() {
    let mut e = engine();
    let ack = e.apply(
        &cue(
            1,
            "a",
            CueOp::TakeOut {
                output: "out-0".into(),
                layer: "Bug".into(),
            },
        ),
        &Empty,
        0,
        false,
    );
    // The operator asked for that layer to be empty, and it is empty. An error
    // here would train them to ignore errors.
    assert_eq!(ack.result, CueResult::Applied);
}

#[test]
fn reconfiguring_takes_the_assignments_layer_order() {
    let mut e = engine(); // starts as [Lower Third, Bug]
    e.configure(
        &[OutputAssignment {
            id: "out-0".into(),
            mode: "1920x1080".into(),
            color: Default::default(),
            layers: vec!["Fullscreen".into(), "Lower Third".into(), "Bug".into()],
        }],
        (60000, 1001),
    );
    // Layer order IS compositing order, so the assignment's order has to win.
    // Appending unknown layers instead — which is what this did first — put
    // Fullscreen ABOVE Lower Third and would have covered it.
    let names: Vec<&str> = e.outputs[0]
        .layers
        .iter()
        .map(|l| l.name.as_str())
        .collect();
    assert_eq!(names, vec!["Fullscreen", "Lower Third", "Bug"]);
}

#[test]
fn reconfiguring_does_not_evict_what_is_on_air() {
    let mut e = engine();
    // ClearAll is enough to prove the mechanism without needing fonts: it
    // leaves the layers in place, and a real occupant would move the same way.
    e.configure(
        &[OutputAssignment {
            id: "out-0".into(),
            mode: "1920x1080".into(),
            color: Default::default(),
            layers: vec!["Ticker".into(), "Lower Third".into()],
        }],
        (60000, 1001),
    );
    let out = &e.outputs[0];
    assert_eq!(out.layers.len(), 2);
    // Bug is gone from the stack, so it is gone from the node. Keeping a layer
    // the assignment no longer lists would leave a graphic on air that nothing
    // can address any more.
    assert!(!out.layers.iter().any(|l| l.name == "Bug"));
    assert!(out.layers.iter().any(|l| l.name == "Lower Third"));
}

#[test]
fn the_state_report_is_the_engines_own_answer() {
    let mut e = engine();
    e.gen = 7;
    e.apply(&cue(3, "a", clear_all()), &Empty, 0, false);

    let s = e.state_report();
    assert_eq!(s.gen, 7);
    assert_eq!(s.last_applied_seq, 3);
    assert_eq!(s.outputs.len(), 1);
    assert_eq!(s.outputs[0].layers.len(), 2);
    assert!(s.outputs[0].layers.iter().all(|l| l.occupant.is_none()));
    // 59.94, not 60. The distinction is the entire reason fps is rational
    // everywhere else in this codebase.
    assert!(
        (s.outputs[0].fps - 59.9400_6).abs() < 0.001,
        "{}",
        s.outputs[0].fps
    );
}

#[test]
fn phase_names_match_the_wire() {
    // The Show view switches on these strings, so a rename here silently stops
    // the browser highlighting what is live.
    assert_eq!(Phase::In.as_str(), "in");
    assert_eq!(Phase::Hold.as_str(), "hold");
    assert_eq!(Phase::Out.as_str(), "out");
}

// ---------------------------------------------------------------------------
// Fan-out (ADR 0018)
// ---------------------------------------------------------------------------

fn dest(output: &str, layer: &str, rev: &str) -> pmg_proto::Destination {
    pmg_proto::Destination {
        output: output.into(),
        layer: layer.into(),
        template_rev: rev.into(),
        mode: "auto".into(),
        blank: false,
    }
}

fn take_to(seq: u64, id: &str, dests: Vec<pmg_proto::Destination>) -> Cue {
    cue(
        seq,
        id,
        CueOp::Take {
            item: "item-1".into(),
            output: dests[0].output.clone(),
            layer: dests[0].layer.clone(),
            destinations: dests,
        },
    )
}

/// A destination naming a layer the node does not have must refuse the WHOLE
/// take, not fire the half it understood. Half a graphic on air with no error
/// is worse than none.
#[test]
fn a_fan_out_naming_an_unknown_layer_fires_nothing() {
    let mut e = engine();
    let before = e.state_report();

    let ack = e.apply(
        &take_to(
            1,
            "press",
            vec![
                dest("out-0", "Lower Third", "blake3:aaa"),
                dest("out-0", "No Such Layer", "blake3:aaa"),
            ],
        ),
        &Empty,
        0,
        false,
    );
    assert_eq!(ack.result, CueResult::Rejected);
    assert_eq!(
        e.state_report().outputs[0].layers.len(),
        before.outputs[0].layers.len(),
        "and nothing moved"
    );
    for l in &e.state_report().outputs[0].layers {
        assert!(l.occupant.is_none(), "no layer was left occupied");
    }
}

/// A blank destination needs no template, which is the distinctive thing about
/// it: it renders nothing, so it demands nothing.
///
/// Asserted as a CONTRAST rather than as "the layer ended up empty" — this
/// Library owns no templates, so an empty layer proves nothing on its own. The
/// same destination fires when blank and is refused when not.
#[test]
fn a_blank_destination_needs_no_template_where_a_real_one_does() {
    let mut e = engine();

    let mut blank = dest("out-0", "Lower Third", "");
    blank.blank = true;
    let cleared = e.apply(&take_to(1, "blank", vec![blank]), &Empty, 0, false);
    assert_eq!(
        cleared.result,
        CueResult::Applied,
        "an instruction to clear that feed, carried out"
    );

    // The same place, the same cue shape, rendering something: refused, because
    // the template is not staged.
    let real = e.apply(
        &take_to(2, "real", vec![dest("out-0", "Lower Third", "blake3:aaa")]),
        &Empty,
        0,
        false,
    );
    assert_eq!(real.result, CueResult::Rejected);

    let layers = e.state_report().outputs[0].layers.clone();
    let l = layers.iter().find(|l| l.name == "Lower Third").unwrap();
    assert!(l.occupant.is_none());
}

/// on_request destinations wait to be asked for. A take carrying only those
/// would put nothing on air, and saying so beats reporting success.
#[test]
fn a_take_of_only_on_request_destinations_is_refused() {
    let mut e = engine();
    let mut later = dest("out-0", "Lower Third", "blake3:aaa");
    later.mode = "on_request".into();

    let ack = e.apply(&take_to(1, "press", vec![later]), &Empty, 0, false);
    assert_eq!(ack.result, CueResult::Rejected);
    assert_eq!(ack.reason, Some(RejectReason::NotReady));
}
