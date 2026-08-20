//! `SceneDoc` types, canonical CBOR + JSON mirror, validation, migration, and
//! `template_rev` hashing.
//!
//! Spec: `docs/specs/scene-format.md` — that document is normative and this
//! crate is the implementation. Where they disagree, the spec is right.
//! Decision: ADR 0005.
//!
//! In the **pure set**. This crate is compiled to WASM for the Design canvas and
//! natively for `pmgctl` and the node, and there is exactly one of it — a
//! template compiled on a laptop and one compiled on a build server must be
//! byte-identical.
//!
//! # M0 status
//!
//! Version-compatibility checking is implemented, because it is small, fully
//! specified, and guards the rule that matters most operationally. The document
//! model, canonical CBOR encoding, and migration chain are M1.

#![forbid(unsafe_code)]

pub mod bind;
pub mod cbor;
pub mod doc;
pub mod fixture;
pub mod migrate;
pub mod validate;

pub use doc::*;

/// The format version this build of the reader implements.
pub const READER_MAJOR: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FormatVersion {
    pub major: u32,
    pub minor: u32,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SceneError {
    /// Raised at **assignment** time, never at take time.
    ///
    /// The whole value of this error is *when* it fires. A node that discovers
    /// it cannot read a template at 10:15 on Sunday has failed; one that says so
    /// at 10am Saturday has done its job. See `docs/specs/scene-format.md` §9.1.
    #[error("template needs a reader of at least v{needs}, this node reads v{has}")]
    TooNew { needs: u32, has: u32 },

    #[error("duplicate node id {0:?} — ids must be unique and are never reused")]
    DuplicateNodeId(String),

    #[error("timeline {timeline:?} targets node {node:?}, which does not exist")]
    DanglingTrack { timeline: String, node: String },

    #[error("binding references field {0:?}, which the document does not declare")]
    UnknownField(String),

    #[error("format is {0:?}, expected \"pmg.scene\"")]
    WrongFormat(String),

    #[error("node {node:?} has a gradient with {why}")]
    BadGradient { node: String, why: String },
}

/// Can this reader open the document?
///
/// Called during assignment reconciliation, before anything is armable.
pub fn check_readable(min_reader: u32, reader_major: u32) -> Result<(), SceneError> {
    if min_reader > reader_major {
        return Err(SceneError::TooNew {
            needs: min_reader,
            has: reader_major,
        });
    }
    Ok(())
}

/// The fixed, total set of binding transforms.
///
/// Every operation is defined on every input and cannot fail, because this
/// evaluates on the render thread at air time. Adding a variant is a `minor`
/// version bump; adding *evaluation* — arithmetic, conditionals, scripting — is
/// a different format and needs its own ADR (ADR 0005 §Alternatives rejected).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Transform {
    Trim,
    Upper,
    Lower,
    Title,
    Truncate(usize),
    Ellipsize(usize),
    Pad(usize),
}

impl Transform {
    /// Apply the transform. Total by construction — no input produces an error.
    ///
    /// Counts are in grapheme clusters conceptually; M0 approximates with
    /// `char`s. Replacing this with a real segmenter is an M1 task and must not
    /// change the totality property.
    pub fn apply(&self, s: &str) -> String {
        match self {
            Transform::Trim => s.trim().to_string(),
            Transform::Upper => s.to_uppercase(),
            Transform::Lower => s.to_lowercase(),
            Transform::Title => s
                .split_whitespace()
                .map(|w| {
                    let mut c = w.chars();
                    match c.next() {
                        Some(f) => {
                            f.to_uppercase().collect::<String>() + &c.as_str().to_lowercase()
                        }
                        None => String::new(),
                    }
                })
                .collect::<Vec<_>>()
                .join(" "),
            Transform::Truncate(n) => s.chars().take(*n).collect(),
            Transform::Ellipsize(n) => {
                if s.chars().count() <= *n {
                    s.to_string()
                } else if *n == 0 {
                    String::new()
                } else {
                    let kept: String = s.chars().take(n.saturating_sub(1)).collect();
                    format!("{kept}…")
                }
            }
            Transform::Pad(n) => {
                let len = s.chars().count();
                if len >= *n {
                    s.to_string()
                } else {
                    format!("{s}{}", " ".repeat(n - len))
                }
            }
        }
    }
}

/// Assertions that gate a release. Called by `pmgctl --selftest`.
///
/// These are the format's load-bearing guarantees. `docs/TESTING.md` says they
/// must not be deleted or weakened, and this is where that is enforced.
pub fn selftest() -> Result<(), String> {
    // --- Version gating, in both directions -------------------------------
    check_readable(1, READER_MAJOR).map_err(|e| e.to_string())?;
    match check_readable(READER_MAJOR + 1, READER_MAJOR) {
        Err(SceneError::TooNew { needs, has })
            if needs == READER_MAJOR + 1 && has == READER_MAJOR => {}
        other => return Err(format!("expected TooNew, got {other:?}")),
    }

    // --- Transforms are total ---------------------------------------------
    let hostile = [
        "",
        "   ",
        "a",
        "日本語のテキスト",
        "🎬🎬🎬",
        "\u{200B}zero width",
    ];
    let transforms = [
        Transform::Trim,
        Transform::Upper,
        Transform::Lower,
        Transform::Title,
        Transform::Truncate(0),
        Transform::Truncate(3),
        Transform::Ellipsize(0),
        Transform::Ellipsize(1),
        Transform::Ellipsize(40),
        Transform::Pad(10),
    ];
    for s in hostile {
        for t in &transforms {
            let _ = t.apply(s); // must not panic — that is the assertion
        }
    }
    if Transform::Ellipsize(40).apply("Pastor Dan") != "Pastor Dan" {
        return Err("ellipsize altered a string under the limit".into());
    }
    let cut = Transform::Ellipsize(5).apply("Pastor Dan Whitfield");
    if !cut.ends_with('…') || cut.chars().count() != 5 {
        return Err(format!("ellipsize produced {cut:?}"));
    }

    let doc = fixture::lower_third();

    // --- The reference document validates ---------------------------------
    let warnings = validate::validate(&doc).map_err(|e| format!("fixture invalid: {e}"))?;
    // It animates only transforms, so there must be no layout_animated warning.
    if let Some(w) = warnings.iter().find(|w| w.code == "layout_animated") {
        return Err(format!("fixture animates layout: {}", w.detail));
    }

    // --- CBOR round-trip is lossless --------------------------------------
    //
    // If this fails, template_rev is not a stable identity and every guarantee
    // built on pinned revisions is false.
    let bytes = cbor::to_canonical(&doc).map_err(|e| e.to_string())?;
    let back: SceneDoc = cbor::from_canonical(&bytes).map_err(|e| e.to_string())?;
    if back != doc {
        return Err("CBOR round-trip changed the document".into());
    }
    if !cbor::is_canonical(&bytes) {
        return Err("encoder did not produce canonical CBOR".into());
    }

    // --- Hashing is stable ------------------------------------------------
    let a = validate::template_rev(&doc).map_err(|e| e.to_string())?;
    let b = validate::template_rev(&back).map_err(|e| e.to_string())?;
    if a != b {
        return Err(format!("template_rev is not stable: {a} vs {b}"));
    }
    if !a.starts_with("blake3:") || a.len() != 7 + 64 {
        return Err(format!("template_rev has the wrong shape: {a}"));
    }

    // --- The JSON mirror round-trips too ----------------------------------
    let json = serde_json::to_string(&doc).map_err(|e| e.to_string())?;
    let from_json: SceneDoc = serde_json::from_str(&json).map_err(|e| e.to_string())?;
    if from_json != doc {
        return Err("JSON mirror round-trip changed the document".into());
    }
    if validate::template_rev(&from_json).map_err(|e| e.to_string())? != a {
        return Err("a document that went through the JSON mirror hashes differently".into());
    }

    // --- visible_if removes the title when empty --------------------------
    //
    // The behaviour the responsive lower third exists for.
    let node = doc.node("title").ok_or("fixture lost its title node")?;
    let cond = node.visible_if.as_ref().ok_or("title has no visible_if")?;
    let with = [("title".to_string(), "Lead Pastor".to_string())]
        .into_iter()
        .collect();
    let without = [("title".to_string(), "  ".to_string())]
        .into_iter()
        .collect();
    if !bind::visible(cond, &with) || bind::visible(cond, &without) {
        return Err("visible_if does not collapse an empty title".into());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selftest_passes() {
        selftest().unwrap();
    }

    #[test]
    fn a_too_new_template_is_refused() {
        assert!(check_readable(2, 1).is_err());
        assert!(check_readable(1, 1).is_ok());
        assert!(
            check_readable(1, 2).is_ok(),
            "a newer reader opens older documents"
        );
    }
}
