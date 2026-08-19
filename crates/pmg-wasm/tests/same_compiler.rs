//! The designer and the CLI must compile a template to the same bytes.
//!
//! This is ADR 0001's central claim, and it is cheap to assert: `design.rs`
//! compiles natively as well as to WASM, so the browser's code path can be
//! checked in CI without a browser. What this cannot check is that the *WASM*
//! build agrees — that is the parity harness's job, and it is why the harness
//! exists. But a divergence introduced in this module would be caught here
//! first, which is where it is cheapest to find.

use pmg_wasm::design;

fn fixture() -> String {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/templates/lower-third.json");
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

#[test]
fn design_compiles_to_the_same_revision_as_pmgctl() {
    let json = fixture();
    let compiled = design::compile(&json).expect("compile");

    // Independently: what `pmg_scene` itself produces, which is exactly what
    // `pmgctl scene compile` writes.
    let doc: pmg_scene::doc::SceneDoc = serde_json::from_str(&json).unwrap();
    let (bytes, manifest) = pmg_scene::validate::compile(&doc).unwrap();

    assert_eq!(
        compiled.scene, bytes,
        "the canonical CBOR must be identical"
    );
    assert_eq!(compiled.template_rev, manifest.template_rev);

    // And the value the browser actually produced, checked in the repo's own
    // fixture, so a change to the encoder cannot quietly move every revision id.
    let recorded: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../testdata/templates/lower-third.manifest.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        compiled.template_rev,
        recorded["template_rev"].as_str().unwrap()
    );
}

#[test]
fn arming_needs_a_font_and_says_so() {
    // There is no system font database, on purpose: a template naming a system
    // font would render one way on a designer's laptop and another on a booth
    // Mac, silently. With no face at all, arming fails rather than quietly
    // rendering nothing.
    let Err(err) = design::arm(&fixture(), "{}", &[], &Default::default()) else {
        panic!("arming with no font must fail");
    };
    assert!(!err.is_empty(), "a failure must explain itself");
}

#[test]
fn a_template_that_is_not_a_template_fails_with_a_reason() {
    let Err(e) = design::compile("{\"nope\":true}") else {
        panic!("that is not a template");
    };
    assert!(e.contains("not a template"), "{e}");
}
