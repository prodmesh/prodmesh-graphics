//! GPI, OSC and MIDI mapping — the parts that need no hardware.
//!
//! The listeners themselves need a desk, a relay box or a MIDI interface. What
//! is testable here is everything that decides WHAT a signal means, which is
//! also where the bugs that matter live: a bouncing contact firing four takes,
//! or a malformed packet reaching the engine.

use pmg_node::surface::{cue_for, osc_address, Debounce, Mapping, Trigger};

fn osc(addr: &str) -> Vec<u8> {
    // OSC pads each part to a 4-byte boundary with nulls.
    let mut v = addr.as_bytes().to_vec();
    v.push(0);
    while !v.len().is_multiple_of(4) {
        v.push(0);
    }
    v.extend_from_slice(b",\0\0\0");
    v
}

#[test]
fn an_osc_address_is_read_without_its_arguments() {
    // A trigger is a button. What it does lives in the mapping, so the
    // arguments are none of this parser's business.
    assert_eq!(osc_address(&osc("/pmg/take/lower-third")).as_deref(), Some("/pmg/take/lower-third"));
}

#[test]
fn a_bundle_or_a_malformed_packet_is_refused() {
    // A bundle carries a timetag, which is a scheduling promise this node does
    // not make — the same reason `Cue.at` is rejected rather than ignored.
    assert_eq!(osc_address(b"#bundle\0"), None);
    // No address at all, no terminator, and binary noise on the port.
    assert_eq!(osc_address(b""), None);
    assert_eq!(osc_address(b"/no-terminator"), None);
    assert_eq!(osc_address(&[b'/', 0x01, 0x02, 0x00]), None);
}

#[test]
fn a_bouncing_contact_is_one_action() {
    // A relay closing produces a burst of edges over a few milliseconds, and by
    // the cue-id rule each is a genuinely distinct action. Without this a
    // footswitch takes the same graphic four times.
    let mut d = Debounce::default();
    assert!(d.accept("/pmg/take"));
    assert!(!d.accept("/pmg/take"));
    assert!(!d.accept("/pmg/take"));
}

#[test]
fn two_different_buttons_at_once_are_two_actions() {
    // Debounce is per trigger, not global. Two operators, or one hand on two
    // switches, must not be collapsed into one.
    let mut d = Debounce::default();
    assert!(d.accept("/pmg/take"));
    assert!(d.accept("/pmg/clear"));
}

#[test]
fn a_trigger_becomes_an_ordinary_cue() {
    let t = Trigger {
        op: "take".into(),
        item: Some("item-1".into()),
        output: Some("out-0".into()),
        layer: Some("Lower Third".into()),
    };
    let c = cue_for(&t, "gpi", 7).expect("a take");
    assert_eq!(c.cue_id, "gpi-7");
    // seq 0 — not part of the server's ordering. Any other value would be
    // deduped against the server's stream, which is the bug the node API
    // verification found.
    assert_eq!(c.seq, 0);
    assert!(c.op.is_deadline_bearing());
}

#[test]
fn a_take_with_no_item_is_refused_rather_than_guessed() {
    // A mapping missing its item is a typo in a JSON file, and inventing an
    // item here would put an arbitrary graphic on air from a footswitch.
    let t = Trigger { op: "take".into(), item: None, output: None, layer: None };
    assert!(cue_for(&t, "gpi", 1).is_none());
}

#[test]
fn an_op_these_surfaces_cannot_send_is_refused() {
    // `update` needs bound data and `timeline` needs a name and an action. A
    // contact closure has neither, so mapping one is a mistake worth naming
    // rather than a cue that half-works.
    for op in ["update", "timeline", "nonsense"] {
        let t = Trigger { op: op.into(), item: Some("i".into()), output: None, layer: None };
        assert!(cue_for(&t, "osc", 1).is_none(), "{op} should be refused");
    }
}

#[test]
fn a_missing_mapping_file_is_normal_and_a_broken_one_is_not() {
    let dir = std::env::temp_dir().join(format!("pmg-surf-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    // Most nodes have no GPI box. Absent is not an error.
    let _ = std::fs::remove_file(dir.join("surfaces.json"));
    assert!(Mapping::load(&dir).unwrap().osc.is_empty());

    // But a file that exists and does not parse IS. A typo must not silently
    // leave a footswitch inert — that gets discovered by pressing it during a
    // service.
    std::fs::write(dir.join("surfaces.json"), b"{ not json").unwrap();
    assert!(Mapping::load(&dir).is_err());

    std::fs::write(
        dir.join("surfaces.json"),
        br#"{"osc":{"/pmg/clear":{"op":"clear_all","output":"out-0"}}}"#,
    )
    .unwrap();
    let m = Mapping::load(&dir).unwrap();
    assert_eq!(m.osc.len(), 1);
    assert_eq!(m.osc["/pmg/clear"].op, "clear_all");
    std::fs::remove_dir_all(&dir).ok();
}
