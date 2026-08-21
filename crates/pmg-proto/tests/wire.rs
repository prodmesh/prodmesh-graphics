//! The wire contract, tested as a contract.
//!
//! Two ends speak this protocol and only one of them is written in Rust. So
//! these tests do two jobs: check the Rust round trip, and **emit a fixture the
//! JavaScript server's tests decode**, so a change to either side that breaks
//! the other fails somewhere rather than nowhere.
//!
//! The fixture is checked in. Regenerating it is the deliberate act of changing
//! the protocol.

use pmg_proto::*;
use std::collections::BTreeMap;

fn sample_cue() -> Cue {
    Cue {
        cue_id: "01JAQ8Z3K7V2N5R9T1W4Y6B8C0".into(),
        seq: 8813,
        epoch: 19,
        deadline_ms: Some(DEFAULT_TAKE_DEADLINE_MS),
        at: None,
        op: CueOp::Take {
            item: "01JBQ8Z3K7V2N5R9T1W4Y6B8C0".into(),
            output: "out-0".into(),
            layer: "Lower Third".into(),
            destinations: Vec::new(),
        },
    }
}

fn sample_hello() -> Hello {
    Hello {
        proto_ver: PROTO_VER,
        node_id: "01JNODE0000000000000000000".into(),
        boot_id: "01JBOOT0000000000000000000".into(),
        name: "Booth-1".into(),
        version: "0.1.0".into(),
        assignment_gen: Some(41),
        last_applied_seq: 8812,
        api_port: 8520,
        preview_port: 8521,
        capabilities: Capabilities {
            outputs: vec![OutputCapability {
                device: "DeckLink Quad 2 (0)".into(),
                kind: "decklink".into(),
                index: 0,
                modes: vec!["1080p5994".into(), "1080i5994".into()],
                supports_key_fill: true,
                paired_port: Some(1),
                has_genlock: true,
                genlock_locked: false,
                ref_format: None,
                audio_pairs: 8,
            }],
            gpu: Some(GpuCapability {
                adapter: "Apple M3 Max".into(),
                backend: "Metal".into(),
                max_texture_2d: 16384,
            }),
            decoders: vec!["prores".into(), "hap".into()],
            max_layers: 8,
            scene_reader_major: SCENE_READER_MAJOR,
        },
    }
}

#[test]
fn messages_round_trip() {
    let msgs = vec![
        ToNode::Welcome(Welcome {
            server_id: "01JSRV0000000000000000000".into(),
            epoch: 19,
            resume_from_seq: Some(8813),
            time_ref: TimeRef {
                utc_ms: 1_786_487_930_986,
                monotonic_ns: 42,
            },
        }),
        ToNode::Cue(sample_cue()),
        ToNode::AssignmentOffer {
            gen: 42,
            digest: "blake3:aa".into(),
        },
        ToNode::Resync {
            reason: "gap exceeded the replay ring".into(),
        },
    ];
    for m in msgs {
        let back: ToNode = decode(&encode(&m).unwrap()).unwrap();
        assert_eq!(back, m);
    }

    let hello = ToServer::Hello(sample_hello());
    assert_eq!(decode::<ToServer>(&encode(&hello).unwrap()).unwrap(), hello);
}

#[test]
fn arm_and_preload_carry_no_deadline_expectation() {
    // A deadline exists to stop a graphic appearing four seconds late. Arming
    // is invisible, so being late costs nothing and rejecting it would be
    // strictness with no purpose behind it.
    let arm = CueOp::Arm {
        item: "i".into(),
        output: "o".into(),
        layer: "l".into(),
    };
    assert!(!arm.is_deadline_bearing());
    assert!(!CueOp::Preload { item: "i".into() }.is_deadline_bearing());

    assert!(CueOp::TakeOut {
        output: "o".into(),
        layer: "l".into()
    }
    .is_deadline_bearing());
    assert!(CueOp::ClearAll { output: "o".into() }.is_deadline_bearing());
}

#[test]
fn every_visible_op_names_its_output() {
    // Routing to the right render thread depends on this, and a `None` here
    // would silently send a take to no output at all.
    for op in [
        CueOp::Take {
            item: "i".into(),
            output: "out-1".into(),
            layer: "l".into(),
            destinations: Vec::new(),
        },
        CueOp::Update {
            // A rebind with no duration takes DEFAULT_TWEEN_MS; the field only
            // travels when a cue asks for something else.
            tween_ms: None,
            output: "out-1".into(),
            layer: "l".into(),
            data: BTreeMap::new(),
        },
        CueOp::ClearAll {
            output: "out-1".into(),
        },
        CueOp::Timeline {
            output: "out-1".into(),
            layer: "l".into(),
            name: "In".into(),
            action: TimelineAction::Goto { t_ms: 200 },
        },
    ] {
        assert_eq!(op.output(), Some("out-1"), "{op:?}");
    }
}

#[test]
fn numbers_bind_without_a_trailing_point_zero() {
    // A score of 3 must read "3". `format!("{}", 3.0f64)` gives "3", but the
    // path through an f64 is exactly where a stray ".0" creeps in, and it is
    // the kind of thing nobody notices until it is on a scoreboard.
    assert_eq!(DataValue::Number(3.0).as_binding_str(), "3");
    assert_eq!(DataValue::Number(-7.0).as_binding_str(), "-7");
    assert_eq!(DataValue::Number(1.5).as_binding_str(), "1.5");
    assert_eq!(
        DataValue::Text("Pastor Dan".into()).as_binding_str(),
        "Pastor Dan"
    );
}

/// Write the fixture the JavaScript side decodes.
///
/// Run with `cargo test -p pmg-proto -- --ignored write_wire_fixture` after a
/// deliberate protocol change. It is ignored by default so a normal test run
/// never rewrites the thing the other side is being checked against — a fixture
/// that regenerates itself proves only that the code agrees with itself.
#[test]
#[ignore]
fn write_wire_fixture() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/proto")
        .canonicalize()
        .unwrap_or_else(|_| {
            let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/proto");
            std::fs::create_dir_all(&p).unwrap();
            p
        });
    std::fs::create_dir_all(&dir).unwrap();

    std::fs::write(
        dir.join("cue.cbor"),
        encode(&ToNode::Cue(sample_cue())).unwrap(),
    )
    .unwrap();
    std::fs::write(
        dir.join("hello.cbor"),
        encode(&ToServer::Hello(sample_hello())).unwrap(),
    )
    .unwrap();
    std::fs::write(
        dir.join("ack.cbor"),
        encode(&ToServer::CueAck(CueAck {
            seq: 8813,
            cue_id: "01JAQ8Z3K7V2N5R9T1W4Y6B8C0".into(),
            result: CueResult::Applied,
            reason: None,
            fired_at_frame: Some(1_841_944),
            at_utc: 1_786_487_930_986,
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(
        dir.join("reject.cbor"),
        encode(&ToServer::CueAck(CueAck {
            seq: 8814,
            cue_id: "01JAQ8Z3K7V2N5R9T1W4Y6B8C1".into(),
            result: CueResult::Rejected,
            reason: Some(RejectReason::DeadlinePassed),
            fired_at_frame: None,
            at_utc: 1_786_487_931_486,
        }))
        .unwrap(),
    )
    .unwrap();
    eprintln!("wrote wire fixtures to {}", dir.display());
}

// ---------------------------------------------------------------------------
// The other direction, which is where the nastier bug lived.
// ---------------------------------------------------------------------------

fn js_fixture(name: &str) -> Vec<u8> {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/proto")
        .join(name);
    std::fs::read(&p)
        .unwrap_or_else(|e| panic!("{}: {e} — run `npm run wire:fixtures`", p.display()))
}

/// Decode frames the **JavaScript server** encoded.
///
/// This test exists because of a specific failure. cbor-x writes an integral JS
/// number above 2^32 as a **float64**, and serde refuses a float where a `u64`
/// is declared — so `Ping { t_ms }` and `Welcome.time_ref.utc_ms`, both built
/// from `Date.now()`, were undecodable. The node logged
/// "invalid type: floating point, expected integer" once a second, never
/// decoded a single `Welcome`, and therefore never learned the server's epoch —
/// while otherwise appearing to run perfectly.
///
/// The Rust→JS fixtures could not catch it: they only ever tested one
/// direction. **A wire contract needs a fixture each way.**
#[test]
fn frames_from_the_javascript_server_decode() {
    match decode::<ToNode>(&js_fixture("js-ping.cbor")).expect("ping") {
        ToNode::Ping { t_ms } => assert_eq!(t_ms, 1_786_487_930_986),
        other => panic!("expected Ping, got {other:?}"),
    }

    match decode::<ToNode>(&js_fixture("js-welcome.cbor")).expect("welcome") {
        ToNode::Welcome(w) => {
            assert_eq!(w.epoch, 19);
            assert_eq!(w.resume_from_seq, Some(8813));
            // Both above 2^32, which is precisely where the encoder switched to
            // a float and the decode failed.
            assert_eq!(w.time_ref.utc_ms, 1_786_487_930_986);
            assert_eq!(w.time_ref.monotonic_ns, 123_456_789_012_345);
        }
        other => panic!("expected Welcome, got {other:?}"),
    }

    match decode::<ToNode>(&js_fixture("js-cue.cbor")).expect("cue") {
        ToNode::Cue(c) => {
            assert_eq!(c.seq, 8813);
            assert_eq!(c.at, None, "reserved, and always null in v1");
            assert_eq!(
                c.op,
                CueOp::Take {
                    item: "01JB".into(),
                    output: "out-0".into(),
                    layer: "Lower Third".into(),
                    // A single-destination cue sends no list, so a fixture
                    // recorded before ADR 0018 decodes unchanged.
                    destinations: Vec::new(),
                }
            );
        }
        other => panic!("expected Cue, got {other:?}"),
    }

    match decode::<ToNode>(&js_fixture("js-assignment.cbor")).expect("assignment") {
        ToNode::Assignment(a) => {
            assert_eq!(a.gen, 42);
            assert_eq!(a.outputs[0].layers, vec!["Lower Third".to_string()]);
            assert_eq!(a.templates[0].min_reader, 1);
            let data = &a.items[0].data;
            assert_eq!(data["name"], DataValue::Text("Pastor Dan Whitfield".into()));
            // An integral JSON number must survive as a Number, not become a
            // string or an int the f64 field cannot hold.
            assert_eq!(data["score"].as_binding_str(), "3");
        }
        other => panic!("expected Assignment, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Readiness, once an Item can be in more than one place — ADR 0018.
// ---------------------------------------------------------------------------

fn dest(layer: &str, ready: bool) -> DestinationReadiness {
    DestinationReadiness {
        output: "out-0".into(),
        layer: layer.into(),
        template_rev: "blake3:aa".into(),
        ready,
        missing: if ready {
            Vec::new()
        } else {
            vec!["blake3:aa".into()]
        },
        unreadable: None,
    }
}

#[test]
fn one_ready_destination_is_enough_to_arm() {
    // ADR 0018 relaxing ADR 0006, and the whole reason it had to: all-or-nothing
    // means an un-staged asset on the overflow feed stops the lower third
    // reaching the programme feed, which is the feed people are watching.
    let i = ItemReadiness {
        item: "i".into(),
        destinations: vec![dest("Lower Third", true), dest("Wall", false)],
    };
    assert!(i.is_armable());
    assert!(
        i.is_partial(),
        "armable and incomplete is a state of its own"
    );
}

#[test]
fn nothing_ready_is_not_partial() {
    // "Partial" has to mean some of it works. Reporting it here would put
    // "it will be incomplete" under a button that cannot fire at all.
    let i = ItemReadiness {
        item: "i".into(),
        destinations: vec![dest("Lower Third", false), dest("Wall", false)],
    };
    assert!(!i.is_armable());
    assert!(!i.is_partial());
}

#[test]
fn every_destination_ready_is_not_partial() {
    let i = ItemReadiness {
        item: "i".into(),
        destinations: vec![dest("Lower Third", true), dest("Wall", true)],
    };
    assert!(i.is_armable());
    assert!(!i.is_partial());
}

#[test]
fn an_item_with_no_destinations_is_not_armable() {
    // A node that was assigned nothing for this Item must not report it
    // takeable. `any()` over an empty list is false, which is the answer we
    // want — this test is here so it stays that answer.
    let i = ItemReadiness {
        item: "i".into(),
        destinations: Vec::new(),
    };
    assert!(!i.is_armable());
    assert!(!i.is_partial());
}

#[test]
fn readiness_round_trips_on_the_wire() {
    let r = ToServer::Ready(Ready {
        assignment_gen: 42,
        items: vec![ItemReadiness {
            item: "i".into(),
            destinations: vec![dest("Lower Third", true), dest("Wall", false)],
        }],
    });
    assert_eq!(decode::<ToServer>(&encode(&r).unwrap()).unwrap(), r);
}
