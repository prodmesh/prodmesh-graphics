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

    // --- Timers (§3.7, format 1.4, ADR 0022) ------------------------------
    //
    // Tabular figures stop being advice the moment a number changes on its own:
    // proportional digits have different widths, so a content-sized box
    // breathes every second and centred type jitters left and right. The spec
    // has warned about `tnum` since §3.3; a timer is where it becomes real, and
    // the node that needs it is nameable here.
    let timers: BTreeSet<&str> = doc
        .fields
        .iter()
        .filter(|f| f.ty == FieldType::Timer)
        .map(|f| f.key.as_str())
        .collect();
    // A font style this reader does not know is kept and drawn upright. Say so
    // at publish, where a typo ("itallic") can still be fixed, rather than on
    // air where it just looks like the slant was forgotten.
    let known_style = |s: &Option<String>| match s.as_deref() {
        None => true,
        Some(v) => ["normal", "italic", "oblique"]
            .iter()
            .any(|k| v.eq_ignore_ascii_case(k)),
    };
    doc.root.walk(&mut |n| {
        if let Some(Content::Text { font, .. }) = &n.content {
            if !known_style(&font.style) {
                warnings.push(Warning {
                    code: "unknown_font_style",
                    detail: format!(
                        "node {:?} asks for font style {:?}; it will render upright",
                        n.id,
                        font.style.as_deref().unwrap_or_default()
                    ),
                });
            }
        }
    });
    for f in doc.fonts.iter().filter(|f| !known_style(&f.style)) {
        warnings.push(Warning {
            code: "unknown_font_style",
            detail: format!(
                "the {:?} declaration has style {:?}; it will be treated as upright",
                f.family,
                f.style.as_deref().unwrap_or_default()
            ),
        });
    }

    if !timers.is_empty() {
        doc.root.walk(&mut |n| {
            let Some(Content::Text { binding, font, .. }) = &n.content else {
                return;
            };
            let uses_timer = crate::bind::referenced_fields(&binding.expr)
                .iter()
                .any(|k| timers.contains(k.as_str()));
            if uses_timer && !font.features.iter().any(|f| f == "tnum") {
                warnings.push(Warning {
                    code: "timer_needs_tnum",
                    detail: format!(
                        "node {:?} shows a timer in a font that has not asked for tabular \
                         figures — the digits will change width every tick and the text \
                         will jitter",
                        n.id
                    ),
                });
            }
        });
    }

    // A field type this reader does not know renders as the Item's stored value
    // and marks the instance degraded — the same bargain unknown node kinds
    // get, and the reason `FieldType::Other` exists at all (ADR 0022).
    for f in &doc.fields {
        if !f.ty.is_known() {
            warnings.push(Warning {
                code: "unknown_field_type",
                detail: format!(
                    "field {:?} has type {:?}, which this reader cannot evaluate — it will \
                     render as whatever the Item stored",
                    f.key, f.ty
                ),
            });
        }
    }

    // --- Masks (§3.6, format 1.3) -----------------------------------------
    //
    // A mask node's SHAPE is the mask; its paint is meaningless and carrying
    // any would be actively dangerous. A 1.2 reader does not know masks: it
    // draws a mask node as an ordinary box, and an unpainted box draws
    // nothing — so an older node shows the masked content unmasked, which is
    // the degradation we can live with. Paint on the same node turns that into
    // a rectangle drawn across the graphic, which is not.
    //
    // An error rather than a warning, and rather than silently dropping the
    // paint: the designer is in an editor that can fix it now.
    let mut bad_mask: Option<(String, String)> = None;
    let mut note_mask = |id: &str, why: &str| {
        if bad_mask.is_none() {
            bad_mask = Some((id.to_string(), why.to_string()));
        }
    };
    if doc.root.kind == NodeKind::Mask {
        note_mask(
            &doc.root.id,
            "is the document root — a mask masks its siblings, and the root has none",
        );
    }
    doc.root.walk(&mut |n| {
        if n.kind != NodeKind::Mask {
            return;
        }
        let s = &n.style;
        if s.fill.is_some() || s.gradient.is_some() || s.stroke.is_some() || s.shadow.is_some() {
            note_mask(
                &n.id,
                "carries paint — a mask is a shape, and a reader too old to \
                 understand masks would draw that paint straight across the graphic",
            );
        }
        if n.content.is_some() {
            note_mask(
                &n.id,
                "carries content — a mask is a shape and nothing else",
            );
        }
        if !n.children.is_empty() {
            note_mask(
                &n.id,
                "has children — they would be masked by their own parent, which \
                 is a shape with no meaning of its own to draw them into",
            );
        }
    });
    // Nested masks: the inner one would silently replace the outer, and losing
    // a mask shows MORE than intended, which is the wrong direction to fail in.
    fn masks_below(n: &crate::doc::Node, inside_masked: bool, out: &mut Option<String>) {
        let here = n.children.iter().any(|c| c.kind == NodeKind::Mask);
        if here && inside_masked && out.is_none() {
            *out = Some(n.id.clone());
        }
        for c in &n.children {
            masks_below(c, inside_masked || here, out);
        }
    }
    let mut nested = None;
    masks_below(&doc.root, false, &mut nested);
    if let Some(id) = nested {
        note_mask(
            &id,
            "is inside an already-masked subtree — v1 applies one mask at a time, \
             and nesting them would drop the outer one silently",
        );
    }
    if let Some((node, why)) = bad_mask {
        return Err(SceneError::BadMask { node, why });
    }
    // A mask is a 1.3 feature and older readers ignore it. Said at publish, so
    // "it looked right in Design and wrong on the old node" is a sentence
    // nobody has to work out on a Sunday.
    let mut any_mask = false;
    doc.root.walk(&mut |n| {
        if n.kind == NodeKind::Mask {
            any_mask = true;
        }
    });
    if any_mask {
        warnings.push(Warning {
            code: "mask_needs_1_3",
            detail: "this template uses a mask; a reader older than format 1.3 shows \
                     the masked content UNMASKED rather than refusing it"
                .into(),
        });
    }

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
