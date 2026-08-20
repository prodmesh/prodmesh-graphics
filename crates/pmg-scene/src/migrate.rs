//! The migration chain — one function per format version step (§9.3).
//!
//! `migrate` walks a document forward to [`CURRENT`], one step at a time. Each
//! step has a **frozen fixture** in `testdata/migrations/<from>-to-<to>/`; a
//! shipped fixture is never edited, because editing one silently changes what
//! "we could read that version" means.
//!
//! Migrating changes the document's bytes, so re-compiling it produces a NEW
//! `template_rev`. Items pin revisions, so a migration never changes anything
//! on air — moving an Item forward is the revision UI's explicit act, with a
//! diff.

use crate::doc::{FormatVersion, SceneDoc};
use crate::SceneError;

/// The version this writer emits.
pub const CURRENT: FormatVersion = FormatVersion { major: 1, minor: 1 };

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
    Ok(applied)
}

/// 1.0 → 1.1: the version that introduced `style.gradient`.
///
/// Purely a stamp. The field is optional and absent in every 1.0 document, so
/// there is nothing to rewrite — the step exists so the chain, the fixture
/// discipline, and the "migration produces a new revision" flow are exercised
/// by a real version rather than remaining a paragraph in the spec.
fn v1_0_to_v1_1(doc: &mut SceneDoc) {
    doc.format_version = FormatVersion { major: 1, minor: 1 };
}
