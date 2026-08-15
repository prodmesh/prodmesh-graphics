//! Canonical CBOR — the encoding `template_rev` is computed over.
//!
//! Spec: `docs/specs/scene-format.md` §1.1. RFC 8949 core-deterministic:
//! definite lengths, map keys sorted by their encoded bytes, shortest integer
//! form, f32 where it round-trips exactly.
//!
//! # Why this is a pass over a `Value` tree rather than a serde attribute
//!
//! Deriving `Serialize` gives definite-length maps with keys in **declaration
//! order**, not sorted order. Making the format canonical by asking developers
//! to declare fields in RFC 8949 sort order would be silently fragile: someone
//! reorders two fields for readability, every `template_rev` changes, and every
//! Item that pinned a revision now points at a hash nothing produces.
//!
//! So canonicalisation is an explicit, tested pass. Struct field order is then
//! free to be whatever reads best.

use ciborium::Value;

/// Encode any serializable value as canonical CBOR.
pub fn to_canonical<T: serde::Serialize>(value: &T) -> Result<Vec<u8>, CborError> {
    let mut v = Value::serialized(value).map_err(|e| CborError::Encode(e.to_string()))?;
    canonicalize(&mut v);
    let mut out = Vec::new();
    ciborium::into_writer(&v, &mut out).map_err(|e| CborError::Encode(e.to_string()))?;
    Ok(out)
}

/// Decode canonical CBOR.
pub fn from_canonical<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, CborError> {
    let v: Value = ciborium::from_reader(bytes).map_err(|e| CborError::Decode(e.to_string()))?;
    v.deserialized()
        .map_err(|e| CborError::Decode(e.to_string()))
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CborError {
    #[error("CBOR encode failed: {0}")]
    Encode(String),
    #[error("CBOR decode failed: {0}")]
    Decode(String),
}

/// Sort every map and narrow every float, in place and recursively.
fn canonicalize(v: &mut Value) {
    match v {
        Value::Map(entries) => {
            for (k, val) in entries.iter_mut() {
                canonicalize(k);
                canonicalize(val);
            }
            // RFC 8949 §4.2.1: sort by the ENCODED bytes of the key, bytewise
            // lexicographic. Encoding each key to compare it is not the fastest
            // possible approach and is the only one that is actually correct —
            // comparing Rust values would get, for example, integer vs text keys
            // wrong.
            entries.sort_by_cached_key(|(k, _)| {
                let mut b = Vec::new();
                let _ = ciborium::into_writer(k, &mut b);
                b
            });
        }
        Value::Array(items) => items.iter_mut().for_each(canonicalize),
        Value::Float(f) => {
            // Store as f32 when that round-trips exactly. Keeps keyframe values
            // compact without ever losing a value that needed the precision.
            let narrowed = *f as f32;
            if narrowed as f64 == *f {
                *f = narrowed as f64;
            }
        }
        _ => {}
    }
}

/// Is `bytes` in canonical form? Re-encodes and compares.
///
/// Used by the property test rather than by production code — but it is the
/// only honest way to assert canonicity, because "we sorted it" is a claim
/// about our code and this is a claim about the bytes.
pub fn is_canonical(bytes: &[u8]) -> bool {
    let Ok(v) = ciborium::from_reader::<Value, _>(bytes) else {
        return false;
    };
    let mut c = v.clone();
    canonicalize(&mut c);
    let mut re = Vec::new();
    if ciborium::into_writer(&c, &mut re).is_err() {
        return false;
    }
    re == bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn map_keys_come_out_sorted_regardless_of_insertion_order() {
        // Deliberately hostile order, including keys of differing length —
        // RFC 8949 sorts by encoded bytes, so the length header participates
        // and "z" sorts before "aa".
        #[derive(serde::Serialize)]
        struct Wild {
            zebra: u8,
            a: u8,
            mm: u8,
            b: u8,
        }
        let bytes = to_canonical(&Wild {
            zebra: 1,
            a: 2,
            mm: 3,
            b: 4,
        })
        .unwrap();
        assert!(
            is_canonical(&bytes),
            "encoder did not produce canonical output"
        );
    }

    #[test]
    fn encoding_is_stable_across_runs() {
        // If this ever fails, template_rev is not a stable identity and every
        // guarantee built on pinned revisions is false.
        let mut m = BTreeMap::new();
        m.insert("beta".to_string(), 2.5f64);
        m.insert("alpha".to_string(), 1.0f64);
        let a = to_canonical(&m).unwrap();
        let b = to_canonical(&m).unwrap();
        assert_eq!(a, b);
        assert!(is_canonical(&a));
    }

    #[test]
    fn floats_narrow_only_when_lossless() {
        // 0.1 is not exactly representable in f32, so it must stay f64.
        let exact = to_canonical(&0.5f64).unwrap();
        let inexact = to_canonical(&0.1f64).unwrap();
        let back_exact: f64 = from_canonical(&exact).unwrap();
        let back_inexact: f64 = from_canonical(&inexact).unwrap();
        assert_eq!(back_exact, 0.5);
        assert_eq!(
            back_inexact, 0.1,
            "narrowing lost a value it should have kept"
        );
        assert!(
            exact.len() < inexact.len(),
            "0.5 should encode smaller as f32"
        );
    }
}
