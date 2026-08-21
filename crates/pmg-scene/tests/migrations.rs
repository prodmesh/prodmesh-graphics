//! The migration chain against its frozen fixtures (§9.3, conformance rule 7).
//!
//! One directory per step in `testdata/migrations/`. A shipped fixture is
//! never edited — if a step is wrong, a new step is added — so these tests are
//! the durable meaning of "this reader could migrate that version". Each
//! fixture certifies its ONE step; the chain test certifies that the steps
//! compose to [`migrate::CURRENT`].

use pmg_scene::doc::SceneDoc;
use pmg_scene::{cbor, migrate};
use std::path::PathBuf;

fn fixture(step: &str, file: &str) -> SceneDoc {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/migrations")
        .join(step)
        .join(file);
    serde_json::from_str(&std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{p:?}: {e}")))
        .unwrap_or_else(|e| panic!("{p:?} does not parse: {e}"))
}

/// One step, one frozen fixture, compared as canonical BYTES — the bytes are
/// what `template_rev` hashes, so byte equality is the claim that matters.
fn step_matches(step: &str, f: fn(&mut SceneDoc)) {
    let mut doc = fixture(step, "input.json");
    let expected = fixture(step, "expected.json");
    f(&mut doc);
    assert_eq!(
        cbor::to_canonical(&doc).unwrap(),
        cbor::to_canonical(&expected).unwrap(),
        "step {step}: the migrated document is byte-for-byte the frozen expectation"
    );
}

#[test]
fn each_step_matches_its_frozen_fixture() {
    step_matches("1.0-to-1.1", migrate::v1_0_to_v1_1);
    step_matches("1.1-to-1.2", migrate::v1_1_to_v1_2);
    step_matches("1.2-to-1.3", migrate::v1_2_to_v1_3);
}

#[test]
fn the_chain_composes_to_current() {
    let mut doc = fixture("1.0-to-1.1", "input.json");
    let applied = migrate::migrate(&mut doc).expect("a 1.0 document migrates");
    assert_eq!(applied, vec!["1.0-to-1.1", "1.1-to-1.2", "1.2-to-1.3"]);
    assert_eq!(doc.format_version, migrate::CURRENT);
}

#[test]
fn a_current_document_migrates_to_nothing() {
    let mut doc = fixture("1.2-to-1.3", "expected.json");
    let before = cbor::to_canonical(&doc).unwrap();
    let applied = migrate::migrate(&mut doc).expect("current is current");
    assert!(applied.is_empty(), "no steps to take");
    assert_eq!(
        cbor::to_canonical(&doc).unwrap(),
        before,
        "and no bytes changed"
    );
}

#[test]
fn a_newer_major_is_refused_not_guessed_at() {
    let mut doc = fixture("1.0-to-1.1", "input.json");
    doc.format_version = pmg_scene::doc::FormatVersion { major: 2, minor: 0 };
    assert!(
        migrate::migrate(&mut doc).is_err(),
        "guessing at a newer major's semantics is how a document renders wrong instead of not at all"
    );
}
