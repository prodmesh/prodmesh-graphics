//! The format's load-bearing guarantees.
//!
//! `docs/TESTING.md` names three property tests that "carry unusual weight and
//! must not be deleted or weakened". This file is where they live.

use pmg_scene::{cbor, doc::*, fixture, validate};

// ---------------------------------------------------------------------------
// 1. CBOR <-> JSON round-trip
// ---------------------------------------------------------------------------

#[test]
fn cbor_round_trip_is_lossless() {
    let doc = fixture::lower_third();
    let bytes = cbor::to_canonical(&doc).unwrap();
    let back: SceneDoc = cbor::from_canonical(&bytes).unwrap();
    assert_eq!(back, doc);
}

#[test]
fn json_mirror_round_trip_is_lossless_and_hashes_the_same() {
    // The JSON mirror exists for diffs, git, and hand-editing. If a document
    // that went through it hashed differently, every template edited by hand
    // would silently become a new revision.
    let doc = fixture::lower_third();
    let json = serde_json::to_string_pretty(&doc).unwrap();
    let back: SceneDoc = serde_json::from_str(&json).unwrap();
    assert_eq!(back, doc);
    assert_eq!(
        validate::template_rev(&doc).unwrap(),
        validate::template_rev(&back).unwrap()
    );
}

#[test]
fn encoding_is_canonical_and_stable() {
    let doc = fixture::lower_third();
    let a = cbor::to_canonical(&doc).unwrap();
    let b = cbor::to_canonical(&doc).unwrap();
    assert_eq!(a, b, "encoding is not deterministic");
    assert!(cbor::is_canonical(&a), "encoding is not canonical");
}

// ---------------------------------------------------------------------------
// 2. Unknown-field preservation — the forward-compat guarantee (ADR 0005)
// ---------------------------------------------------------------------------

#[test]
fn unknown_node_keys_survive_a_read_write_cycle() {
    // An older designer must be able to open, edit, and re-save a document
    // written by a newer one WITHOUT silently deleting what it does not
    // understand. That is the difference between a format that survives two
    // versions in the field and one that does not.
    let doc = fixture::lower_third();
    let mut v = serde_json::to_value(&doc).unwrap();

    // Inject keys this reader knows nothing about, as a future version would.
    v["root"]["children"][0]["glow_radius"] = serde_json::json!(12.5);
    v["root"]["children"][0]["future_block"] = serde_json::json!({ "a": 1, "b": [2, 3] });

    let read: SceneDoc = serde_json::from_value(v).unwrap();
    let band = read.node("band").expect("band survived");
    assert!(
        band.unknown.contains_key("glow_radius"),
        "unknown key was dropped on read: {:?}",
        band.unknown.keys().collect::<Vec<_>>()
    );
    assert!(band.unknown.contains_key("future_block"));

    // And it must come back out on write.
    let rewritten = serde_json::to_value(&read).unwrap();
    assert_eq!(rewritten["root"]["children"][0]["glow_radius"], 12.5);
    assert_eq!(rewritten["root"]["children"][0]["future_block"]["b"][1], 3);

    // Sanity: the fixture itself carries no unknowns, so the test is really
    // measuring the injection rather than pre-existing noise.
    doc.root.walk(&mut |n| assert!(n.unknown.is_empty()));
}

#[test]
fn an_unknown_node_kind_is_preserved_but_refuses_to_render() {
    // Preserve on write, refuse to draw. Rendering nothing and looking fine is
    // the dangerous behaviour (scene-format.md §3.4).
    let doc = fixture::lower_third();
    let mut v = serde_json::to_value(&doc).unwrap();
    v["root"]["children"][0]["kind"] = serde_json::json!("hologram");

    let read: SceneDoc = serde_json::from_value(v).unwrap();
    let band = read.node("band").unwrap();
    assert_eq!(band.kind, NodeKind::Other("hologram".into()));
    assert!(
        !band.kind.is_renderable(),
        "an unknown kind must not claim to be drawable"
    );

    let rewritten = serde_json::to_value(&read).unwrap();
    assert_eq!(rewritten["root"]["children"][0]["kind"], "hologram");

    // And validation warns rather than failing — the document is still openable.
    let warnings = validate::validate(&read).unwrap();
    assert!(warnings.iter().any(|w| w.code == "unknown_kind"));
}

#[test]
fn document_level_ext_survives() {
    let doc = fixture::lower_third();
    let mut v = serde_json::to_value(&doc).unwrap();
    v["ext"] = serde_json::json!({ "authored_by": "a future version" });
    let read: SceneDoc = serde_json::from_value(v).unwrap();
    assert!(read.ext.contains_key("authored_by"));
    let out = serde_json::to_value(&read).unwrap();
    assert_eq!(out["ext"]["authored_by"], "a future version");
}

// ---------------------------------------------------------------------------
// 3. Validation rules
// ---------------------------------------------------------------------------

#[test]
fn a_too_new_document_is_refused() {
    let mut doc = fixture::lower_third();
    doc.min_reader = pmg_scene::READER_MAJOR + 1;
    // The whole value of this error is WHEN it fires: at assignment, on a
    // Saturday — not at take time on a Sunday.
    assert!(matches!(
        validate::validate(&doc),
        Err(pmg_scene::SceneError::TooNew { .. })
    ));
}

#[test]
fn duplicate_node_ids_are_rejected() {
    // Ids are the addressing scheme for every timeline track, so a duplicate
    // makes an animation ambiguous.
    let doc = fixture::lower_third();
    let mut v = serde_json::to_value(&doc).unwrap();
    v["root"]["children"][0]["children"][1]["id"] = serde_json::json!("name");
    let dup: SceneDoc = serde_json::from_value(v).unwrap();
    assert!(matches!(
        validate::validate(&dup),
        Err(pmg_scene::SceneError::DuplicateNodeId(_))
    ));
}

#[test]
fn a_binding_to_an_undeclared_field_is_caught_at_publish() {
    let doc = fixture::lower_third();
    let mut v = serde_json::to_value(&doc).unwrap();
    v["root"]["children"][0]["children"][0]["content"]["binding"]["expr"] =
        serde_json::json!("${nmae}"); // the typo that would blank a lower third
    let typo: SceneDoc = serde_json::from_value(v).unwrap();
    assert!(matches!(
        validate::validate(&typo),
        Err(pmg_scene::SceneError::UnknownField(f)) if f == "nmae"
    ));
}

#[test]
fn a_dangling_track_warns_but_does_not_break_the_document() {
    // Deleting a layer must not make a template unopenable.
    let doc = fixture::lower_third();
    let mut v = serde_json::to_value(&doc).unwrap();
    v["timelines"][0]["tracks"][0]["target"] =
        serde_json::json!("deleted-node/transform.translate.x");
    let orphaned: SceneDoc = serde_json::from_value(v).unwrap();

    let warnings = validate::validate(&orphaned).expect("still openable");
    assert!(warnings.iter().any(|w| w.code == "dangling_track"));
}

#[test]
fn animating_layout_warns() {
    let doc = fixture::lower_third();
    let mut v = serde_json::to_value(&doc).unwrap();
    v["timelines"][0]["tracks"][0]["target"] = serde_json::json!("band/layout.width");
    let bad: SceneDoc = serde_json::from_value(v).unwrap();

    let warnings = validate::validate(&bad).unwrap();
    assert!(
        warnings.iter().any(|w| w.code == "layout_animated"),
        "animating layout relayouts every frame and must be flagged"
    );
}

// ---------------------------------------------------------------------------
// Identity
// ---------------------------------------------------------------------------

#[test]
fn editing_anything_produces_a_new_revision() {
    // Revisions are immutable and content-addressed. Items pin one, so this is
    // what makes republishing a template safe for anything already on air.
    let doc = fixture::lower_third();
    let before = validate::template_rev(&doc).unwrap();

    let mut edited = doc.clone();
    edited.name = "Lower Third — Speaker (v2)".into();
    let after = validate::template_rev(&edited).unwrap();

    assert_ne!(before, after);
    assert_eq!(
        before,
        validate::template_rev(&doc).unwrap(),
        "hash is not stable"
    );
}

#[test]
fn the_manifest_carries_what_the_server_validates() {
    // The server never parses a SceneDoc — it validates this and nothing else
    // (ADR 0004).
    let doc = fixture::lower_third();
    let (bytes, manifest) = validate::compile(&doc).unwrap();

    assert!(!bytes.is_empty());
    assert_eq!(manifest.template_rev, validate::template_rev(&doc).unwrap());
    assert_eq!(
        manifest.fields.len(),
        2,
        "Item validation needs the field schema"
    );
    assert_eq!(manifest.timelines, vec!["In", "Out"]);
    assert_eq!(manifest.min_reader, 1);
    // The font's bytes are an asset, so a node knows to fetch them.
    assert!(manifest.assets.iter().any(|a| a.starts_with("blake3:")));
}

// ---------------------------------------------------------------------------
// Defaults
// ---------------------------------------------------------------------------

#[test]
fn a_default_style_is_fully_opaque() {
    // `#[serde(default = "one")]` fills a MISSING FIELD during deserialization.
    // `Default::default()` uses f32::default() = 0.0. Deriving Default here
    // made every node built as `Style { fill: .., ..Default::default() }`
    // invisible, and the renderer drew a frame of nothing — which looks exactly
    // like a broken shader.
    assert_eq!(Style::default().opacity, 1.0);
    assert_eq!(
        Style {
            fill: Some(Rgba {
                rgba: [1.0, 1.0, 1.0, 1.0],
                token: None
            }),
            ..Default::default()
        }
        .opacity,
        1.0
    );
}

#[test]
fn a_default_transform_is_the_identity() {
    // Same trap: scale must default to 1, not 0. A scale of zero collapses the
    // node to a point.
    let t = Transform2D::default();
    assert_eq!(t.scale, [1.0, 1.0]);
    assert_eq!(t.translate, [0.0, 0.0]);
    assert_eq!(t.rotate, 0.0);
}

#[test]
fn every_node_in_the_fixture_is_visible_by_default() {
    // The end-to-end version of the two above: nothing in the reference
    // template should be accidentally transparent or collapsed.
    let doc = fixture::lower_third();
    doc.root.walk(&mut |n| {
        assert!(
            n.style.opacity > 0.0,
            "node {:?} has opacity {}",
            n.id,
            n.style.opacity
        );
        assert!(
            n.transform.scale[0] != 0.0 && n.transform.scale[1] != 0.0,
            "node {:?} has a zero scale",
            n.id
        );
    });
}

#[test]
fn a_blank_document_compiles_and_declares_nothing() {
    // The designer's "New template" starts here, so a blank that fails to
    // publish would be a door that opens onto a wall.
    let doc = SceneDoc::blank("01TESTBLANKDOCUMENT000001", "Untitled", 1920, 1080);
    let (_, manifest) = validate::compile(&doc).expect("a blank document publishes");
    assert!(manifest.fields.is_empty());
    assert!(manifest.assets.is_empty());
    assert!(manifest.timelines.is_empty());
    assert!(manifest.warnings.is_empty(), "{:?}", manifest.warnings);
}
