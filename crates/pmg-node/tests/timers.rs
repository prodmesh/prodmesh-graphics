//! Timers at playout (ADR 0022).
//!
//! The node computes a timer from its own clock, so a countdown keeps counting
//! when the control server dies. These tests pin what that means in the engine:
//! that the picture changes on the second and not on the frame, that the rebind
//! does not morph, that the deadline lands on the boundary, and that a template
//! disappearing under a live clock stops the clock rather than the graphic.

use pmg_node::engine::{LayerEngine, Library};
use pmg_proto::*;
use pmg_scene::doc::*;
use pmg_scene::timer::{TimerFormat, TimerKind, TimerSpec};
use std::collections::BTreeMap;

const TARGET: i64 = 1_755_691_200_000;

/// A boxes-and-text document showing one timer field.
///
/// Text with no font shapes to nothing, which is fine here: these tests are
/// about WHEN the engine re-solves, and the solved instance is the instrument.
fn doc() -> SceneDoc {
    let mut d = SceneDoc::blank("01TESTTIMERNODE0000000001", "Countdown", 1920, 1080);
    d.fields = vec![Field {
        key: "starts_in".into(),
        ty: FieldType::Timer,
        label: "Starts in".into(),
        required: false,
        max_len: None,
        sample: Some("0:00".into()),
    }];
    d.fonts = vec![FontRef {
        family: "Test".into(),
        weight: 400,
        style: None,
        asset: None,
        system_fallback: false,
    }];
    d.root.children = vec![Node {
        id: "count".into(),
        display_name: None,
        kind: NodeKind::Text,
        layout: Layout::default(),
        transform: Transform2D::default(),
        style: Style::default(),
        clip: false,
        visible_if: None,
        content: Some(Content::Text {
            binding: Binding {
                expr: "${starts_in}".into(),
                transform: vec![],
            },
            font: FontSpec {
                family: "Test".into(),
                weight: 400,
                size: 96.0,
                features: vec!["tnum".into()],
                tracking: 0.0,
                line_height: 1.2,
                token: None,
            },
            align: Align::Center,
            max_lines: None,
        }),
        children: vec![],
        unknown: Default::default(),
    }];
    d
}

struct OneItem {
    doc: SceneDoc,
    data: BTreeMap<String, DataValue>,
    /// Set to make the template vanish, as it would if an assignment replaced
    /// it while a clock was live.
    hidden: bool,
}

impl Library for OneItem {
    fn template(&self, rev: &str) -> Option<&SceneDoc> {
        (!self.hidden && rev == "rev-1").then_some(&self.doc)
    }
    fn item(&self, id: &str) -> Option<(&str, &str, &BTreeMap<String, DataValue>)> {
        (id == "item-1").then_some(("Countdown", "rev-1", &self.data))
    }
    fn text_engine(&self) -> anyhow::Result<pmg_text::TextEngine> {
        Ok(pmg_text::TextEngine::new(pmg_text::FontDb::new()))
    }
    fn image(&self, _key: &str) -> Option<(u32, u32, std::sync::Arc<Vec<u8>>)> {
        None
    }
}

fn lib(spec: TimerSpec) -> OneItem {
    let mut data = BTreeMap::new();
    data.insert("starts_in".into(), DataValue::Timer(spec));
    OneItem {
        doc: doc(),
        data,
        hidden: false,
    }
}

fn engine() -> LayerEngine {
    let mut e = LayerEngine::new();
    e.configure(
        &[OutputAssignment {
            id: "out-0".into(),
            mode: "1920x1080".into(),
            color: Default::default(),
            layers: vec!["Graphics".into()],
        }],
        (60000, 1001),
    );
    e
}

/// Take the item at a given wall time.
fn take_at(e: &mut LayerEngine, l: &dyn Library, now_ms: i64) {
    let ack = e.apply(
        &Cue {
            cue_id: "take".into(),
            seq: 1,
            epoch: 1,
            deadline_ms: Some(DEFAULT_TAKE_DEADLINE_MS),
            at: None,
            op: CueOp::Take {
                item: "item-1".into(),
                output: "out-0".into(),
                layer: "Graphics".into(),
                destinations: vec![],
            },
        },
        l,
        now_ms as u64,
        false,
    );
    assert_eq!(ack.result, CueResult::Applied, "{:?}", ack.reason);
}

/// What the layer's text currently reads, from the SOLVED instance — not from
/// the declaration, so this is what a viewer would see.
fn shown(e: &LayerEngine) -> String {
    let o = e.outputs[0].layers[0]
        .occupant
        .as_ref()
        .expect("something on air");
    o.shown
        .get("starts_in")
        .cloned()
        .unwrap_or_else(|| "<none>".into())
}

#[test]
fn a_countdown_ticks_once_a_second_and_not_once_a_frame() {
    let l = lib(TimerSpec::countdown_to(TARGET));
    let mut e = engine();
    // Deliberately NOT on a boundary: starting exactly on one made an earlier
    // version of this test assert that a frame later nothing had changed, when
    // in fact the very next millisecond changed it.
    take_at(&mut e, &l, TARGET - 5_400);
    e.run_timers(&l, TARGET - 5_400, 0);
    assert_eq!(shown(&e), "0:05");

    // A frame later — 16.68 ms — nothing has changed, and the engine says so
    // by rebinding nothing.
    assert!(e.run_timers(&l, TARGET - 5_383, 0).is_empty());
    assert_eq!(shown(&e), "0:05");

    // Cross the boundary and it rebinds exactly once.
    let done = e.run_timers(&l, TARGET - 4_999, 0);
    assert_eq!(done, vec!["out-0/Graphics".to_string()]);
    assert_eq!(shown(&e), "0:04");
}

#[test]
fn the_deadline_lands_on_the_boundary_rather_than_a_second_later_each_time() {
    let l = lib(TimerSpec::countdown_to(TARGET));
    let mut e = engine();
    take_at(&mut e, &l, TARGET - 5_400);
    e.run_timers(&l, TARGET - 5_400, 0);
    // 5.4 s left reads 0:05 and becomes 0:04 the millisecond it drops below
    // 5 s — 401 ms away, not a second.
    assert_eq!(shown(&e), "0:05");
    assert_eq!(e.next_timer_due_ms(), Some(TARGET - 4_999));
}

/// Walking a whole countdown a frame at a time: every second appears exactly
/// once, in order, and the engine rebinds exactly as many times as the display
/// changed. This is the property a clock that stutters or skips would break.
#[test]
fn a_frame_walk_rebinds_once_per_second_and_skips_nothing() {
    let l = lib(TimerSpec::countdown_to(TARGET));
    let mut e = engine();
    let start = TARGET - 10_000 - 137;
    take_at(&mut e, &l, start);

    let mut seen: Vec<String> = Vec::new();
    let mut rebinds = 0;
    for f in 0..660u64 {
        let now = start + (f as i64 * 1000 * 1001) / 60000;
        rebinds += e.run_timers(&l, now, 0).len();
        let s = shown(&e);
        if seen.last().map(|p| p != &s).unwrap_or(true) {
            seen.push(s);
        }
    }
    let expected: Vec<String> = (0..=10).rev().map(|n| format!("0:{n:02}")).collect();
    assert_eq!(seen, expected, "every second, once, in order");
    // Eleven displayed values means eleven rebinds — one per change, none
    // spare. 660 frames went past; 649 of them did nothing.
    assert_eq!(rebinds, expected.len(), "one rebind per change, no more");
}

#[test]
fn a_timer_rebind_does_not_morph() {
    let l = lib(TimerSpec::countdown_to(TARGET));
    let mut e = engine();
    take_at(&mut e, &l, TARGET - 5_000);
    e.run_timers(&l, TARGET - 5_000, 0);
    e.run_timers(&l, TARGET - 4_000, 0);
    // A tween here would start a 400 ms animation on every tick, and the next
    // tick would interrupt it (ADR 0022).
    assert!(
        e.outputs[0].layers[0]
            .occupant
            .as_ref()
            .unwrap()
            .tween
            .is_none(),
        "a clock must not animate between its own numbers"
    );
}

#[test]
fn a_finished_countdown_stops_asking() {
    let l = lib(TimerSpec::countdown_to(TARGET));
    let mut e = engine();
    take_at(&mut e, &l, TARGET - 1_000);
    e.run_timers(&l, TARGET - 1_000, 0);
    assert_eq!(shown(&e), "0:01");

    e.run_timers(&l, TARGET, 0);
    assert_eq!(shown(&e), "0:00");
    // Floored, so there is nothing left to compute and the runtime goes back to
    // its ordinary poll.
    assert_eq!(e.next_timer_due_ms(), None);
    assert!(e.run_timers(&l, TARGET + 60_000, 0).is_empty());
    assert_eq!(
        shown(&e),
        "0:00",
        "and it stays at zero rather than blanking"
    );
}

#[test]
fn elapsed_counts_from_the_take_not_from_the_arm() {
    let l = lib(TimerSpec {
        kind: TimerKind::Elapsed,
        at: None,
        format: TimerFormat::MmSs,
    });
    let mut e = engine();
    take_at(&mut e, &l, TARGET);
    e.run_timers(&l, TARGET, 0);
    assert_eq!(shown(&e), "00:00");
    e.run_timers(&l, TARGET + 65_000, 0);
    assert_eq!(shown(&e), "01:05");
}

/// An Item with no timers costs nothing: no deadline, no rebinds, ever.
#[test]
fn an_item_without_timers_is_never_asked_about() {
    let mut l = lib(TimerSpec::countdown_to(TARGET));
    l.data.clear();
    let mut e = engine();
    take_at(&mut e, &l, TARGET);
    assert!(e.run_timers(&l, TARGET, 0).is_empty());
    assert_eq!(e.next_timer_due_ms(), None);
    assert!(e.run_timers(&l, TARGET + 3_600_000, 0).is_empty());
}

/// The template goes away under a live clock — an assignment replaced it, or a
/// font failed to load. The number stops; the graphic does NOT come off air.
#[test]
fn a_clock_whose_template_vanishes_stops_rather_than_taking_the_graphic_down() {
    let mut l = lib(TimerSpec::countdown_to(TARGET));
    let mut e = engine();
    take_at(&mut e, &l, TARGET - 5_000);
    e.run_timers(&l, TARGET - 5_000, 0);
    assert_eq!(shown(&e), "0:05");

    l.hidden = true;
    let done = e.run_timers(&l, TARGET - 4_000, 0);
    assert!(done.is_empty(), "nothing could be rebound");
    assert!(
        e.outputs[0].layers[0].occupant.is_some(),
        "the graphic stays on air showing the last number it had — taking it \
         down mid-service because a clock could not re-solve is a far bigger \
         failure than a stopped clock"
    );
}

/// NOT an assertion — a measurement, printed. `cargo test -- --nocapture`.
#[test]
fn measure_what_a_tick_costs() {
    let font = [
        "/System/Library/Fonts/Supplemental/Arial.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    ]
    .iter()
    .find_map(|p| std::fs::read(p).ok());
    struct Fonted(OneItem, Option<Vec<u8>>);
    impl Library for Fonted {
        fn template(&self, r: &str) -> Option<&SceneDoc> {
            self.0.template(r)
        }
        fn item(&self, i: &str) -> Option<(&str, &str, &BTreeMap<String, DataValue>)> {
            self.0.item(i)
        }
        fn image(&self, k: &str) -> Option<(u32, u32, std::sync::Arc<Vec<u8>>)> {
            self.0.image(k)
        }
        fn text_engine(&self) -> anyhow::Result<pmg_text::TextEngine> {
            let mut db = pmg_text::FontDb::new();
            if let Some(b) = &self.1 {
                db.add_face(b.clone(), "Test".to_string(), 400)?;
            }
            Ok(pmg_text::TextEngine::new(db))
        }
    }
    let l = Fonted(lib(TimerSpec::countdown_to(TARGET)), font);
    let mut e = engine();
    take_at(&mut e, &l, TARGET - 600_000);
    e.run_timers(&l, TARGET - 600_000, 0);

    let mut worst = 0.0f64;
    let mut total = 0.0f64;
    let n = 300;
    for i in 0..n {
        let now = TARGET - 600_000 + (i as i64 + 1) * 1000;
        let t0 = std::time::Instant::now();
        let done = e.run_timers(&l, now, 0);
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        assert_eq!(done.len(), 1);
        worst = worst.max(ms);
        total += ms;
    }
    // And the cost of a pass that changes nothing, which is 59 frames in 60.
    let mut idle = 0.0f64;
    for i in 0..1000 {
        let now = TARGET - 600_000 + 1;
        let t0 = std::time::Instant::now();
        e.run_timers(&l, now + i % 3, 0);
        idle = idle.max(t0.elapsed().as_secs_f64() * 1000.0);
    }
    println!(
        "TIMER TICK: mean {:.3} ms, worst {:.3} ms over {n} ticks; idle pass worst {:.4} ms",
        total / n as f64,
        worst,
        idle
    );
}
