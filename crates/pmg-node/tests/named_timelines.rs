//! Named timelines at playout — `Update`, `Highlight`, `Loop`.
//!
//! `In` and `Out` are the occupant's LIFE; every other timeline is a run
//! layered over it, fired by an explicit cue and blended by the format's one
//! rule: start order, last writer per target wins per frame
//! (`docs/specs/scene-format.md` §6.1, §6.3). These tests pin the engine to
//! that rule, plus the completion behaviors the format reserves — `loop`,
//! `on_complete: hold | evict | goto`.

use pmg_node::engine::{frame_values, LayerEngine, Library, Phase};
use pmg_proto::*;
use pmg_scene::doc::*;
use std::collections::BTreeMap;

/// A boxes-only document — arms with no fonts — whose timelines exercise every
/// completion rule. All durations are multiples of a 100 ms so a 10 ms test
/// tick lands exactly on the edges.
fn doc() -> SceneDoc {
    let chip = Node {
        id: "chip".into(),
        display_name: None,
        kind: NodeKind::Box,
        layout: Layout {
            width: Dimension::Px(100.0),
            height: Dimension::Px(100.0),
            ..Default::default()
        },
        transform: Transform2D::default(),
        style: Style {
            fill: Some(Rgba {
                rgba: [1.0, 0.0, 0.0, 1.0],
            }),
            ..Default::default()
        },
        clip: false,
        visible_if: None,
        content: None,
        children: vec![],
        unknown: Default::default(),
    };
    let root = Node {
        id: "root".into(),
        display_name: None,
        kind: NodeKind::Box,
        layout: Layout {
            width: Dimension::Px(1920.0),
            height: Dimension::Px(1080.0),
            ..Default::default()
        },
        transform: Transform2D::default(),
        style: Style::default(),
        clip: false,
        visible_if: None,
        content: None,
        children: vec![chip],
        unknown: Default::default(),
    };

    let tl = |name: &str, keys: [(f32, f32); 2], looping: bool, on_complete: OnComplete| Timeline {
        name: name.into(),
        duration_ms: 100.0,
        on_complete,
        looping,
        tracks: vec![Track {
            target: "chip/transform.translate.x".into(),
            blend: Blend::Replace,
            keys: keys
                .into_iter()
                .map(|(t, v)| pmg_anim::Key {
                    t,
                    v,
                    ease: pmg_anim::Ease::Linear,
                })
                .collect(),
        }],
    };

    SceneDoc {
        format: "pmg.scene".into(),
        format_version: FormatVersion { major: 1, minor: 0 },
        min_reader: 1,
        id: "01TESTNAMEDTIMELINES00001".into(),
        name: "Named timelines".into(),
        canvas: Canvas {
            width: 1920,
            height: 1080,
            safe: None,
        },
        fields: vec![],
        fonts: vec![],
        assets: vec![],
        root,
        timelines: vec![
            // In lands the chip at x = 0; Hold holds it there.
            tl("In", [(0.0, -400.0), (100.0, 0.0)], false, OnComplete::Hold),
            tl(
                "Out",
                [(0.0, 0.0), (100.0, -400.0)],
                false,
                OnComplete::Hold,
            ),
            // The flourish: pushes x to 50 and holds it there when done.
            tl(
                "Highlight",
                [(0.0, 0.0), (100.0, 50.0)],
                false,
                OnComplete::Hold,
            ),
            // The shimmer: loops forever until stopped.
            tl(
                "Shimmer",
                [(0.0, 0.0), (100.0, 10.0)],
                true,
                OnComplete::Hold,
            ),
            // The authored exit: an Out by another name.
            tl(
                "Bye",
                [(0.0, 0.0), (100.0, 900.0)],
                false,
                OnComplete::Evict,
            ),
            // The chain: hands off to Highlight when it finishes.
            tl(
                "Chain",
                [(0.0, 0.0), (100.0, 5.0)],
                false,
                OnComplete::Goto("Highlight".into()),
            ),
        ],
        ext: Default::default(),
    }
}

/// A library holding exactly one template and one item bound to it.
struct OneItem {
    doc: SceneDoc,
    data: BTreeMap<String, DataValue>,
}

impl Library for OneItem {
    fn template(&self, rev: &str) -> Option<&SceneDoc> {
        (rev == "rev-1").then_some(&self.doc)
    }
    fn item(&self, id: &str) -> Option<(&str, &str, &BTreeMap<String, DataValue>)> {
        (id == "item-1").then_some(("Chip", "rev-1", &self.data))
    }
    fn text_engine(&self) -> anyhow::Result<pmg_text::TextEngine> {
        Ok(pmg_text::TextEngine::new(pmg_text::FontDb::new()))
    }
    fn image(&self, _key: &str) -> Option<(u32, u32, std::sync::Arc<Vec<u8>>)> {
        None
    }
}

fn lib() -> OneItem {
    OneItem {
        doc: doc(),
        data: BTreeMap::new(),
    }
}

/// An engine at an even 100 fps, so ticks are exactly 10 ms and land on key
/// times rather than near them.
fn engine() -> LayerEngine {
    let mut e = LayerEngine::new();
    e.configure(
        &[OutputAssignment {
            id: "out-0".into(),
            mode: "1920x1080".into(),
            color: Default::default(),
            layers: vec!["Graphics".into()],
        }],
        (100, 1),
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

fn timeline_cue(seq: u64, id: &str, name: &str, action: TimelineAction) -> Cue {
    cue(
        seq,
        id,
        CueOp::Timeline {
            output: "out-0".into(),
            layer: "Graphics".into(),
            name: name.into(),
            action,
        },
    )
}

/// Take item-1 and run its In to the end, leaving the occupant on Hold.
fn on_air(e: &mut LayerEngine, l: &OneItem) {
    let ack = e.apply(
        &cue(
            1,
            "take",
            CueOp::Take {
                item: "item-1".into(),
                output: "out-0".into(),
                layer: "Graphics".into(),
                destinations: vec![],
            },
        ),
        l,
        0,
        false,
    );
    assert_eq!(ack.result, CueResult::Applied, "{:?}", ack.reason);
    for _ in 0..12 {
        e.tick("out-0");
    }
    assert_eq!(
        e.outputs[0].layers[0].occupant.as_ref().unwrap().phase,
        Phase::Hold
    );
}

fn x_of(e: &LayerEngine) -> f32 {
    let o = e.outputs[0].layers[0].occupant.as_ref().unwrap();
    *frame_values(o)
        .get("chip/transform.translate.x")
        .expect("the track evaluates")
}

#[test]
fn a_named_run_overlays_the_held_in_and_wins_per_target() {
    let l = lib();
    let mut e = engine();
    on_air(&mut e, &l);
    assert_eq!(x_of(&e), 0.0, "Hold is In's final state");

    let ack = e.apply(
        &timeline_cue(2, "hl", "Highlight", TimelineAction::Play),
        &l,
        0,
        false,
    );
    assert_eq!(ack.result, CueResult::Applied, "{:?}", ack.reason);

    for _ in 0..5 {
        e.tick("out-0");
    }
    // 50 ms into a 100 ms ramp to 50: the run's value, not Hold's 0.
    assert_eq!(x_of(&e), 25.0);

    // The state report says it is running — that is the operator's feedback.
    let s = e.state_report();
    let occ = s.outputs[0].layers[0].occupant.as_ref().unwrap();
    assert_eq!(occ.named, vec!["Highlight".to_string()]);
}

#[test]
fn play_restarts_a_run_rather_than_stacking_a_second() {
    let l = lib();
    let mut e = engine();
    on_air(&mut e, &l);
    e.apply(
        &timeline_cue(2, "a", "Highlight", TimelineAction::Play),
        &l,
        0,
        false,
    );
    for _ in 0..5 {
        e.tick("out-0");
    }
    e.apply(
        &timeline_cue(3, "b", "Highlight", TimelineAction::Play),
        &l,
        0,
        false,
    );
    let o = e.outputs[0].layers[0].occupant.as_ref().unwrap();
    assert_eq!(o.named.len(), 1);
    assert_eq!(x_of(&e), 0.0, "the double-tap highlights NOW, from the top");
}

#[test]
fn hold_keeps_the_final_values_and_stops_reporting_motion() {
    let l = lib();
    let mut e = engine();
    on_air(&mut e, &l);
    e.apply(
        &timeline_cue(2, "hl", "Highlight", TimelineAction::Play),
        &l,
        0,
        false,
    );
    for _ in 0..20 {
        e.tick("out-0");
    }
    // Finished 100 ms ago: still asserting its end value — that is Hold.
    assert_eq!(x_of(&e), 50.0);
    let s = e.state_report();
    let occ = s.outputs[0].layers[0].occupant.as_ref().unwrap();
    assert!(occ.named.is_empty(), "held is a look, not motion");
}

#[test]
fn stop_removes_the_run_and_the_base_shows_through() {
    let l = lib();
    let mut e = engine();
    on_air(&mut e, &l);
    e.apply(
        &timeline_cue(2, "hl", "Highlight", TimelineAction::Play),
        &l,
        0,
        false,
    );
    for _ in 0..20 {
        e.tick("out-0");
    }
    assert_eq!(x_of(&e), 50.0);
    e.apply(
        &timeline_cue(3, "stop", "Highlight", TimelineAction::Stop),
        &l,
        0,
        false,
    );
    assert_eq!(x_of(&e), 0.0, "the base phase is back");
}

#[test]
fn a_looping_timeline_wraps_and_keeps_running() {
    let l = lib();
    let mut e = engine();
    on_air(&mut e, &l);
    e.apply(
        &timeline_cue(2, "sh", "Shimmer", TimelineAction::Play),
        &l,
        0,
        false,
    );
    for _ in 0..25 {
        e.tick("out-0");
    }
    // 250 ms into a looping 100 ms ramp to 10: wrapped to 50 ms = 5.
    assert_eq!(x_of(&e), 5.0);
    let s = e.state_report();
    let occ = s.outputs[0].layers[0].occupant.as_ref().unwrap();
    assert_eq!(
        occ.named,
        vec!["Shimmer".to_string()],
        "a loop never finishes"
    );
}

#[test]
fn evict_is_an_authored_exit() {
    let l = lib();
    let mut e = engine();
    on_air(&mut e, &l);
    e.apply(
        &timeline_cue(2, "bye", "Bye", TimelineAction::Play),
        &l,
        0,
        false,
    );
    for _ in 0..11 {
        e.tick("out-0");
    }
    assert!(
        e.outputs[0].layers[0].occupant.is_none(),
        "on_complete: evict leaves the layer"
    );
}

#[test]
fn goto_chains_into_the_named_timeline() {
    let l = lib();
    let mut e = engine();
    on_air(&mut e, &l);
    e.apply(
        &timeline_cue(2, "ch", "Chain", TimelineAction::Play),
        &l,
        0,
        false,
    );
    // 100 ms of Chain, then 50 ms into the Highlight it hands off to.
    for _ in 0..15 {
        e.tick("out-0");
    }
    assert_eq!(x_of(&e), 25.0, "Highlight took over at Chain's end");
}

#[test]
fn a_timeline_the_template_does_not_have_is_refused_by_name() {
    let l = lib();
    let mut e = engine();
    on_air(&mut e, &l);
    let ack = e.apply(
        &timeline_cue(2, "no", "Sparkle", TimelineAction::Play),
        &l,
        0,
        false,
    );
    assert_eq!(ack.result, CueResult::Rejected);
    assert_eq!(ack.reason, Some(RejectReason::NoSuchTimeline));
}

#[test]
fn a_take_out_clears_the_flourish_so_out_can_leave() {
    let l = lib();
    let mut e = engine();
    on_air(&mut e, &l);
    e.apply(
        &timeline_cue(2, "hl", "Highlight", TimelineAction::Play),
        &l,
        0,
        false,
    );
    e.apply(
        &cue(
            3,
            "out",
            CueOp::TakeOut {
                output: "out-0".into(),
                layer: "Graphics".into(),
            },
        ),
        &l,
        0,
        false,
    );
    let o = e.outputs[0].layers[0].occupant.as_ref().unwrap();
    assert_eq!(o.phase, Phase::Out);
    assert!(
        o.named.is_empty(),
        "a held run overriding Out's tracks would pin the graphic on screen"
    );
}
