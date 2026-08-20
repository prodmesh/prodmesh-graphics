//! Binding evaluation.
//!
//! Spec: `docs/specs/scene-format.md` §5.2–5.3.
//!
//! # The one property that matters
//!
//! **Evaluation is total.** Every function here is defined on every input and
//! cannot fail, panic, or error. That is not a stylistic preference: this runs
//! on the render thread at air time, so a binding that could fail would be a
//! template capable of taking the output down.
//!
//! An unknown field resolves to the empty string. A malformed placeholder is
//! left as literal text. A transform on nonsense returns nonsense, quietly.
//! There is deliberately no error path to handle, because there is no error.

use crate::doc::{VisibleIf, VisibleOp};
use crate::Transform;
use std::collections::BTreeMap;

/// Item data: field key → value, as typed by an operator in Edit.
pub type Data = BTreeMap<String, String>;

/// Expand `${field}` placeholders and apply the transform chain.
///
/// Cannot fail. See the module note.
pub fn eval(expr: &str, transforms: &[Transform], data: &Data) -> String {
    let mut out = expand(expr, data);
    for t in transforms {
        out = t.apply(&out);
    }
    out
}

/// Substitute `${key}` occurrences. Unknown keys become empty.
///
/// An unterminated `${` is emitted literally rather than swallowing the rest of
/// the string — a designer who typos a placeholder should see the typo on
/// screen, not lose their copy.
fn expand(expr: &str, data: &Data) -> String {
    let mut out = String::with_capacity(expr.len());
    let bytes = expr.as_bytes();
    let mut i = 0usize;

    while i < bytes.len() {
        if bytes[i] == b'$' && i + 1 < bytes.len() && bytes[i + 1] == b'{' {
            match expr[i + 2..].find('}') {
                Some(rel) => {
                    let key = &expr[i + 2..i + 2 + rel];
                    if let Some(v) = data.get(key) {
                        out.push_str(v);
                    }
                    i = i + 2 + rel + 1;
                }
                None => {
                    // No closing brace: literal text, not a swallowed tail.
                    out.push_str(&expr[i..]);
                    break;
                }
            }
        } else {
            let ch = expr[i..].chars().next().unwrap_or('\u{FFFD}');
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out
}

/// Every field key an expression references, in order of appearance.
///
/// Used by validation to catch a typo at publish rather than on air. Evaluation
/// itself never needs this — an unknown key is simply empty.
pub fn referenced_fields(expr: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = expr;
    while let Some(start) = rest.find("${") {
        let after = &rest[start + 2..];
        match after.find('}') {
            Some(end) => {
                out.push(after[..end].to_string());
                rest = &after[end + 1..];
            }
            None => break,
        }
    }
    out
}

/// Should this node participate in layout?
///
/// **False means removed from layout, not merely hidden** — which is what
/// collapses a lower third to one line when there is no title. Total, like
/// everything else here: an unknown field is treated as empty.
/// Resolve field-driven media to concrete content addresses, in place.
///
/// An image or video node whose content names a `field` gets its `asset_key`
/// REPLACED with the field's value — the Item's chosen hash — falling back to
/// the field's sample so the designer previews something. After this pass the
/// renderer's asset lookup needs no notion of fields: a key that starts with
/// `blake3:` IS the address, and every `ImageSource` resolves it directly.
///
/// Called at ARM, by every path that arms — the browser designer, the playout
/// node, the CLI harness — because a resolution done in two of three places is
/// a graphic that previews and then does not play.
pub fn resolve_media_fields(doc: &mut crate::doc::SceneDoc, data: &Data) {
    fn value_for(doc_fields: &[crate::doc::Field], data: &Data, key: &str) -> Option<String> {
        let v = data
            .get(key)
            .cloned()
            .or_else(|| {
                doc_fields
                    .iter()
                    .find(|f| f.key == key)
                    .and_then(|f| f.sample.clone())
            })
            .unwrap_or_default();
        if v.starts_with("blake3:") {
            Some(v)
        } else {
            None
        }
    }
    fn walk(n: &mut crate::doc::Node, fields: &[crate::doc::Field], data: &Data) {
        match &mut n.content {
            Some(crate::doc::Content::Image {
                asset_key,
                field: Some(k),
                ..
            })
            | Some(crate::doc::Content::Video {
                asset_key,
                field: Some(k),
                ..
            }) => {
                // An empty value leaves an unresolvable key, which the
                // renderer reports as DEGRADED — the honest answer for an
                // Item that has not picked its picture yet.
                *asset_key = value_for(fields, data, k).unwrap_or_default();
            }
            _ => {}
        }
        for c in &mut n.children {
            walk(c, fields, data);
        }
    }
    let fields = doc.fields.clone();
    walk(&mut doc.root, &fields, data);
}

pub fn visible(cond: &VisibleIf, data: &Data) -> bool {
    let raw = data.get(&cond.field).map(String::as_str).unwrap_or("");
    match &cond.op {
        VisibleOp::NonEmpty => !raw.trim().is_empty(),
        VisibleOp::Empty => raw.trim().is_empty(),
        VisibleOp::Eq(v) => raw == v,
        VisibleOp::Ne(v) => raw != v,
        // A non-numeric value is simply not greater than anything. No error,
        // no panic — the graphic renders without that element.
        VisibleOp::Gt(n) => raw.trim().parse::<f64>().map(|v| v > *n).unwrap_or(false),
        VisibleOp::Lt(n) => raw.trim().parse::<f64>().map(|v| v < *n).unwrap_or(false),
        VisibleOp::True => matches!(raw.trim(), "true" | "1" | "yes"),
        VisibleOp::False => !matches!(raw.trim(), "true" | "1" | "yes"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(pairs: &[(&str, &str)]) -> Data {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn expands_and_transforms() {
        let d = data(&[("name", "  Pastor Dan  ")]);
        assert_eq!(eval("${name}", &[Transform::Trim], &d), "Pastor Dan");
        assert_eq!(eval("Hi, ${name}!", &[], &d), "Hi,   Pastor Dan  !");
    }

    #[test]
    fn unknown_field_is_empty_not_an_error() {
        let d = data(&[]);
        assert_eq!(eval("${nope}", &[], &d), "");
        assert_eq!(eval("[${nope}]", &[], &d), "[]");
    }

    #[test]
    fn an_unterminated_placeholder_stays_visible() {
        // A designer who typos should SEE the typo, not silently lose the rest
        // of their copy.
        let d = data(&[("name", "Dan")]);
        assert_eq!(eval("${name} and ${broken", &[], &d), "Dan and ${broken");
    }

    #[test]
    fn evaluation_is_total_against_hostile_input() {
        // The assertion is that none of this panics. There is no error path to
        // check because there is no error.
        let hostile = [
            "",
            "${",
            "}",
            "${}",
            "${${}}",
            "$${a}",
            "\u{200B}${a}",
            "🎬${a}🎬",
            "${a}${a}${a}",
            "${日本語}",
        ];
        let all = [
            Transform::Trim,
            Transform::Upper,
            Transform::Lower,
            Transform::Title,
            Transform::Truncate(0),
            Transform::Ellipsize(1),
            Transform::Pad(5),
        ];
        let d = data(&[("a", "x"), ("日本語", "y")]);
        for e in hostile {
            for t in &all {
                let _ = eval(e, std::slice::from_ref(t), &d);
            }
            let _ = eval(e, &all, &d);
        }
    }

    #[test]
    fn visible_if_collapses_the_second_line() {
        // The behaviour the whole feature exists for.
        let cond = VisibleIf {
            field: "title".into(),
            op: VisibleOp::NonEmpty,
        };
        assert!(visible(&cond, &data(&[("title", "Lead Pastor")])));
        assert!(!visible(&cond, &data(&[("title", "")])));
        assert!(
            !visible(&cond, &data(&[("title", "   ")])),
            "whitespace is empty"
        );
        assert!(!visible(&cond, &data(&[])), "a missing field is empty");
    }

    #[test]
    fn numeric_comparisons_do_not_panic_on_text() {
        let c = VisibleIf {
            field: "n".into(),
            op: VisibleOp::Gt(5.0),
        };
        assert!(visible(&c, &data(&[("n", "6")])));
        assert!(!visible(&c, &data(&[("n", "banana")])));
        assert!(!visible(&c, &data(&[])));
    }

    #[test]
    fn referenced_fields_finds_them_all() {
        assert_eq!(referenced_fields("${a} x ${b}"), vec!["a", "b"]);
        assert_eq!(referenced_fields("none here"), Vec::<String>::new());
        assert_eq!(referenced_fields("${unterminated"), Vec::<String>::new());
    }
}
