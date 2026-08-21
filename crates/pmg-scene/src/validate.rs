//! Document validation, and the `template_rev` identity.
//!
//! Split from the model on purpose: the types describe what a document *is*,
//! this describes what makes one *usable*. A document can be structurally valid
//! CBOR and still be nonsense.

use crate::{cbor, doc::*, SceneError, READER_MAJOR};
use std::collections::BTreeSet;

/// A non-fatal finding. Emitted into the compiler manifest and surfaced by the
/// editor; never blocks a publish.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Warning {
    pub code: &'static str,
    pub detail: String,
}

/// The content-addressed identity of a document.
///
/// `template_rev = blake3(canonical CBOR)`. Revisions are immutable: editing a
/// template creates a new one, and an Item pins the revision it was authored
/// against — so republishing never changes anything on air.
pub fn template_rev(doc: &SceneDoc) -> Result<String, cbor::CborError> {
    let bytes = cbor::to_canonical(doc)?;
    Ok(format!("blake3:{}", blake3::hash(&bytes).to_hex()))
}

/// Validate a document.
///
/// Returns hard errors, or the warning list if it is publishable.
pub fn validate(doc: &SceneDoc) -> Result<Vec<Warning>, SceneError> {
    if doc.format != "pmg.scene" {
        return Err(SceneError::WrongFormat(doc.format.clone()));
    }
    crate::check_readable(doc.min_reader, READER_MAJOR)?;

    let mut warnings = Vec::new();

    // Node ids must be unique. They are the addressing scheme for every
    // timeline track, so a duplicate makes an animation ambiguous.
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut duplicate: Option<String> = None;
    doc.root.walk(&mut |n| {
        if !seen.insert(n.id.as_str()) && duplicate.is_none() {
            duplicate = Some(n.id.clone());
        }
    });
    if let Some(id) = duplicate {
        return Err(SceneError::DuplicateNodeId(id));
    }

    // An unknown node kind is preserved, not rejected — but it will render as a
    // magenta placeholder, so say so at publish rather than letting someone
    // discover it on air.
    doc.root.walk(&mut |n| {
        if !n.kind.is_renderable() {
            warnings.push(Warning {
                code: "unknown_kind",
                detail: format!(
                    "node {:?} has kind {:?}, which this reader cannot draw",
                    n.id, n.kind
                ),
            });
        }
    });

    // Gradient stop rules are a compile ERROR, not a warning: a designer made
    // this document in an editor that can fix it now, and a gradient whose
    // stops are out of order evaluates differently between any two readers
    // that clamp differently.
    let mut bad_gradient: Option<(String, String)> = None;
    doc.root.walk(&mut |n| {
        if bad_gradient.is_some() {
            return;
        }
        if let Some(g) = &n.style.gradient {
            let why = if g.stops.len() < 2 || g.stops.len() > 4 {
                Some(format!("{} stops — it needs 2 to 4", g.stops.len()))
            } else if g.stops.iter().any(|s| !(0.0..=1.0).contains(&s.at)) {
                Some("a stop outside 0..=1".to_string())
            } else if g.stops.windows(2).any(|w| w[1].at < w[0].at) {
                Some("stops out of order".to_string())
            } else {
                None
            };
            if let Some(why) = why {
                bad_gradient = Some((n.id.clone(), why));
            }
        }
    });
    if let Some((node, why)) = bad_gradient {
        return Err(SceneError::BadGradient { node, why });
    }

    let fields: BTreeSet<&str> = doc.fields.iter().map(|f| f.key.as_str()).collect();

    // Bindings and visible_if may only reference declared fields. Evaluation is
    // total — an unknown key resolves to empty rather than failing — but a
    // reference to a field that does not exist is almost always a typo, and
    // catching it at publish is much cheaper than a blank lower third.
    let mut bad_field: Option<String> = None;
    doc.root.walk(&mut |n| {
        if let Some(Content::Text { binding, .. }) = &n.content {
            for key in crate::bind::referenced_fields(&binding.expr) {
                if !fields.contains(key.as_str()) && bad_field.is_none() {
                    bad_field = Some(key);
                }
            }
        }
        if let Some(v) = &n.visible_if {
            if !fields.contains(v.field.as_str()) && bad_field.is_none() {
                bad_field = Some(v.field.clone());
            }
        }
        // Field-driven media reference fields the same way bindings do, and an
        // undeclared one is the same typo caught at the same moment.
        if let Some(Content::Image { field: Some(k), .. } | Content::Video { field: Some(k), .. }) =
            &n.content
        {
            if !fields.contains(k.as_str()) && bad_field.is_none() {
                bad_field = Some(k.clone());
            }
        }
    });
    if let Some(f) = bad_field {
        return Err(SceneError::UnknownField(f));
    }

    // A track whose target node does not exist is DROPPED WITH A WARNING, never
    // a hard error — deleting a layer must not make a template unopenable
    // (docs/specs/scene-format.md §6.2).
    let ids: BTreeSet<&str> = seen;
    for tl in &doc.timelines {
        for tr in &tl.tracks {
            if !ids.contains(tr.node_id()) {
                warnings.push(Warning {
                    code: "dangling_track",
                    detail: format!(
                        "timeline {:?} targets {:?}, which no longer exists — the track is ignored",
                        tl.name, tr.target
                    ),
                });
            }
            // Animating layout relayouts every frame; animating transform is
            // free. Not an error, but the editor should say so loudly.
            if tr.animates_layout() {
                warnings.push(Warning {
                    code: "layout_animated",
                    detail: format!(
                        "timeline {:?} animates {:?}, which forces a relayout every frame — \
                         animate transform instead",
                        tl.name, tr.target
                    ),
                });
            }
        }
    }

    Ok(warnings)
}

/// The manifest sidecar the compiler emits.
///
/// **This is all the control server ever validates.** It never parses a
/// SceneDoc — a JavaScript reimplementation of the scene format would be two
/// implementations of the thing that must never disagree (ADR 0004).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Manifest {
    pub template_rev: String,
    pub template_id: String,
    pub name: String,
    pub format_version: FormatVersion,
    pub min_reader: u32,
    pub fields: Vec<Field>,
    pub timelines: Vec<String>,
    pub assets: Vec<String>,
    /// The theme this document's token values were resolved under (ADR 0021),
    /// so "which theme was this compiled under" is answerable from the
    /// manifest the server actually stores.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,
    pub warnings: Vec<ManifestWarning>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ManifestWarning {
    pub code: String,
    pub detail: String,
}

/// Compile a document to its canonical bytes plus the manifest sidecar.
/// The content address of arbitrary bytes, in the family's one form.
///
/// Exposed here because the parity harness must prove both halves were handed
/// the SAME font file, and "same length" is not the same claim. `pmg-cas` has
/// the store; this is just the hash, and it is in the pure set so the browser
/// can compute it too.
pub fn address_of(bytes: &[u8]) -> String {
    format!("blake3:{}", blake3::hash(bytes).to_hex())
}

pub fn compile(doc: &SceneDoc) -> Result<(Vec<u8>, Manifest), SceneError> {
    let warnings = validate(doc)?;
    let bytes = cbor::to_canonical(doc).map_err(|e| SceneError::WrongFormat(e.to_string()))?;
    let rev = format!("blake3:{}", blake3::hash(&bytes).to_hex());

    let mut assets: Vec<String> = doc.assets.iter().map(|a| a.hash.clone()).collect();
    assets.extend(doc.fonts.iter().filter_map(|f| f.asset.clone()));
    assets.sort();
    assets.dedup();

    let manifest = Manifest {
        template_rev: rev,
        template_id: doc.id.clone(),
        name: doc.name.clone(),
        format_version: doc.format_version,
        min_reader: doc.min_reader,
        fields: doc.fields.clone(),
        timelines: doc.timelines.iter().map(|t| t.name.clone()).collect(),
        assets,
        theme: doc.theme.clone(),
        warnings: warnings
            .into_iter()
            .map(|w| ManifestWarning {
                code: w.code.to_string(),
                detail: w.detail,
            })
            .collect(),
    };
    Ok((bytes, manifest))
}
