//! The migration chain — one function per format version step (§9.3).
//!
//! `migrate` walks a document forward to [`CURRENT`], one step at a time. Each
//! step has a **frozen fixture** in `testdata/migrations/<from>-to-<to>/`; a
//! shipped fixture is never edited, because editing one silently changes what
//! "we could read that version" means. The step functions are public for
//! exactly that reason: each fixture certifies ONE step, not wherever the
//! chain happens to end this year.
//!
//! Migrating changes the document's bytes, so re-compiling it produces a NEW
//! `template_rev`. Items pin revisions, so a migration never changes anything
//! on air — moving an Item forward is the revision UI's explicit act, with a
//! diff.

use crate::doc::{FormatVersion, SceneDoc};
use crate::SceneError;

/// The version this writer emits.
pub const CURRENT: FormatVersion = FormatVersion { major: 1, minor: 6 };

/// Walk `doc` forward to [`CURRENT`], returning the names of the steps taken
/// — empty when it was already current.
///
/// A MAJOR version this reader does not know is an error, the same refusal
/// `check_readable` makes at assignment: guessing at semantics a newer format
/// changed is how a document renders wrong instead of not at all.
pub fn migrate(doc: &mut SceneDoc) -> Result<Vec<&'static str>, SceneError> {
    if doc.format_version.major != CURRENT.major {
        return Err(SceneError::TooNew {
            needs: doc.format_version.major,
            has: CURRENT.major,
        });
    }
    let mut applied = Vec::new();
    if doc.format_version.minor == 0 {
        v1_0_to_v1_1(doc);
        applied.push("1.0-to-1.1");
    }
    if doc.format_version.minor == 1 {
        v1_1_to_v1_2(doc);
        applied.push("1.1-to-1.2");
    }
    if doc.format_version.minor == 2 {
        v1_2_to_v1_3(doc);
        applied.push("1.2-to-1.3");
    }
    if doc.format_version.minor == 3 {
        v1_3_to_v1_4(doc);
        applied.push("1.3-to-1.4");
    }
    if doc.format_version.minor == 4 {
        v1_4_to_v1_5(doc);
        applied.push("1.4-to-1.5");
    }
    if doc.format_version.minor == 5 {
        v1_5_to_v1_6(doc);
        applied.push("1.5-to-1.6");
    }
    Ok(applied)
}

/// 1.0 → 1.1: the version that introduced `style.gradient`.
///
/// Purely a stamp. The field is optional and absent in every 1.0 document, so
/// there is nothing to rewrite — the step exists so the chain, the fixture
/// discipline, and the "migration produces a new revision" flow are exercised
/// by a real version rather than remaining a paragraph in the spec.
pub fn v1_0_to_v1_1(doc: &mut SceneDoc) {
    doc.format_version = FormatVersion { major: 1, minor: 1 };
}

/// 1.1 → 1.2: the version that introduced theme tokens (ADR 0021) — `token`
/// provenance on colours and fonts, and the document's `theme` stamp.
///
/// Also purely a stamp: every 1.1 document simply has no tokens.
pub fn v1_1_to_v1_2(doc: &mut SceneDoc) {
    doc.format_version = FormatVersion { major: 1, minor: 2 };
}

/// 1.2 → 1.3: the version where `kind: "mask"` started meaning something.
///
/// A stamp again, and the fallback is why it can be. A 1.2 reader draws a mask
/// node as an unpainted box — which is to say, nothing — and shows the content
/// that should have been masked, unmasked. That is the reason the compiler
/// REFUSES to let a mask node carry a fill, a gradient, a stroke, or a shadow:
/// the moment it carries paint, an older reader draws a rectangle across the
/// graphic instead of quietly revealing too much.
pub fn v1_2_to_v1_3(doc: &mut SceneDoc) {
    doc.format_version = FormatVersion { major: 1, minor: 3 };
}

/// 1.3 → 1.4: timers (ADR 0022) — the `timer` field type, and the `Other`
/// escape hatch that `FieldType` should have had all along.
///
/// A stamp, like the three before it: a 1.3 document simply has no timer
/// fields. The step that is NOT free is the other direction — a 1.3 reader
/// cannot parse a document containing `"type": "timer"` and refuses the
/// template at assignment. That is the last time a field type does this, which
/// is the whole point of adding the escape now.
pub fn v1_3_to_v1_4(doc: &mut SceneDoc) {
    doc.format_version = FormatVersion { major: 1, minor: 4 };
}

/// 1.4 → 1.5: vertical alignment on a text node.
///
/// A stamp, and free in both directions for once. A 1.4 reader ignores
/// `valign` and renders the text at the top of its box, which is what every
/// document before this did — so an older node shows a graphic that is
/// positioned differently rather than one it cannot read. That is the mildest
/// degradation of any version so far, and it is why this did not need the
/// refusal a mask or a timer needs.
pub fn v1_4_to_v1_5(doc: &mut SceneDoc) {
    doc.format_version = FormatVersion { major: 1, minor: 5 };
}

/// 1.5 → 1.6: `font.style` on a text node — italic text.
///
/// A stamp, and a mild one. A 1.5 reader ignores the field and draws the text
/// upright in the same family and weight: the words are right and the slant is
/// missing, which an older node survives. The declaration side (`fonts[].style`)
/// was in the format from 1.0 and simply never read; from 1.6 it decides which
/// face a family resolves to, so an italic declaration is what makes a node
/// stage the italic file.
pub fn v1_5_to_v1_6(doc: &mut SceneDoc) {
    doc.format_version = FormatVersion { major: 1, minor: 6 };
}
