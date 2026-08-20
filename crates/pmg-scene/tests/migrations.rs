//! The migration chain against its frozen fixtures (§9.3, conformance rule 7).
//!
//! One directory per step in `testdata/migrations/`. A shipped fixture is
//! never edited — if a step is wrong, a new step is added — so these tests are
//! the durable meaning of "this reader could migrate that version".

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

#[test]
fn the_1_0_to_1_1_step_matches_its_frozen_fixture() {
    let mut doc = fixture("1.0-to-1.1", "input.json");
    let expected = fixture("1.0-to-1.1", "expected.json");

    let applied = migrate::migrate(&mut doc).expect("a 1.0 document migrates");
    assert_eq!(applied, vec!["1.0-to-1.1"]);

    // Compared as canonical BYTES, not as structs: the bytes are what
    // template_rev hashes, so byte equality is the claim that actually matters.
    assert_eq!(
        cbor::to_canonical(&doc).unwrap(),
        cbor::to_canonical(&expected).unwrap(),
        "the migrated document is byte-for-byte the frozen expectation"
    );
}

#[test]
fn a_current_document_migrates_to_nothing() {
    let mut doc = fixture("1.0-to-1.1", "expected.json");
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
