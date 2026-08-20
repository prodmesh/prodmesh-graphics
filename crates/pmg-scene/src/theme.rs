//! Themes — named tokens a document's colours and fonts can follow (ADR 0021).
//!
//! A theme is a small content-addressed document: a name and a map of tokens.
//! It never travels to a node and the server never interprets it — the
//! COMPILER does, here, and the published template carries the RESOLVED values
//! with each token's name as provenance. That is what keeps `template_rev`
//! naming its pixels: the same revision renders the same everywhere, theme or
//! no theme, and changing a theme is a republish rather than a runtime switch.

use crate::doc::{Content, Node, SceneDoc};
use crate::SceneError;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Theme {
    pub name: String,
    /// Token name → value. Names are free-form; `brand.primary` is the
    /// convention, not a rule.
    pub tokens: BTreeMap<String, TokenValue>,
}

/// What a token can hold in v1: a colour, or a font face. Assets are a later,
/// additive kind (ADR 0021's seam), which is why this is an enum and not two
/// maps.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenValue {
    Color { rgba: [f32; 4] },
    Font { family: String, weight: u16 },
}

impl Theme {
    /// Canonical CBOR — what the CAS hash addresses, computed by the same
    /// encoder that makes `template_rev` mean something.
    pub fn to_canonical(&self) -> Result<Vec<u8>, SceneError> {
        crate::cbor::to_canonical(self).map_err(|e| SceneError::WrongFormat(e.to_string()))
    }

    /// The theme's content address plus the bytes it addresses — one hasher,
    /// the same one behind `template_rev`.
    pub fn address(&self) -> Result<(String, Vec<u8>), SceneError> {
        let bytes = self.to_canonical()?;
        Ok((format!("blake3:{}", blake3::hash(&bytes).to_hex()), bytes))
    }
}

/// Re-resolve every token reference in `doc` from `theme`, overwriting the
/// stored values, and stamp the document with the theme's address.
///
/// Returns how many values changed. A token the theme does not define is an
/// ERROR, not a fallback — the stale value would still render, but a dangling
/// token is a typo worth catching while someone is looking (ADR 0021).
pub fn resolve(doc: &mut SceneDoc, theme: &Theme, theme_hash: &str) -> Result<usize, SceneError> {
    let mut changed = 0usize;
    let mut dangling: Option<String> = None;

    fn walk(n: &mut Node, theme: &Theme, changed: &mut usize, dangling: &mut Option<String>) {
        let mut color = |c: &mut crate::doc::Rgba| {
            if let Some(t) = &c.token {
                match theme.tokens.get(t) {
                    Some(TokenValue::Color { rgba }) => {
                        if c.rgba != *rgba {
                            c.rgba = *rgba;
                            *changed += 1;
                        }
                    }
                    _ if dangling.is_none() => *dangling = Some(t.clone()),
                    _ => {}
                }
            }
        };
        if let Some(f) = &mut n.style.fill {
            color(f);
        }
        if let Some(g) = &mut n.style.gradient {
            for s in &mut g.stops {
                color(&mut s.color);
            }
        }
        if let Some(s) = &mut n.style.stroke {
            color(&mut s.color);
        }
        if let Some(s) = &mut n.style.shadow {
            color(&mut s.color);
        }
        if let Some(Content::Text { font, .. }) = &mut n.content {
            if let Some(t) = &font.token {
                match theme.tokens.get(t) {
                    Some(TokenValue::Font { family, weight }) => {
                        if &font.family != family || font.weight != *weight {
                            font.family = family.clone();
                            font.weight = *weight;
                            *changed += 1;
                        }
                    }
                    _ if dangling.is_none() => *dangling = Some(t.clone()),
                    _ => {}
                }
            }
        }
        for c in &mut n.children {
            walk(c, theme, changed, dangling);
        }
    }
    walk(&mut doc.root, theme, &mut changed, &mut dangling);

    if let Some(token) = dangling {
        return Err(SceneError::UnknownToken {
            token,
            theme: theme.name.clone(),
        });
    }
    if doc.theme.as_deref() != Some(theme_hash) {
        doc.theme = Some(theme_hash.to_string());
        changed += 1;
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::Rgba;

    fn theme() -> Theme {
        Theme {
            name: "Advent".into(),
            tokens: BTreeMap::from([
                (
                    "brand.primary".into(),
                    TokenValue::Color {
                        rgba: [0.8, 0.1, 0.1, 1.0],
                    },
                ),
                (
                    "brand.font".into(),
                    TokenValue::Font {
                        family: "IBM Plex Sans".into(),
                        weight: 600,
                    },
                ),
            ]),
        }
    }

    #[test]
    fn resolve_rewrites_tokened_values_and_stamps_the_theme() {
        let mut doc = crate::fixture::lower_third();
        // Token the band's fill at a stale value.
        let band = &mut doc.root.children[0];
        band.style.fill = Some(Rgba {
            rgba: [0.0, 0.0, 1.0, 1.0],
            token: Some("brand.primary".into()),
        });

        let n = resolve(&mut doc, &theme(), "blake3:themehash").unwrap();
        assert!(n >= 2, "the value and the stamp both changed, got {n}");
        assert_eq!(
            doc.root.children[0].style.fill.as_ref().unwrap().rgba,
            [0.8, 0.1, 0.1, 1.0],
            "the token's current value replaced the stale one"
        );
        assert_eq!(doc.theme.as_deref(), Some("blake3:themehash"));

        // Idempotent: resolving again under the same theme changes nothing.
        assert_eq!(resolve(&mut doc, &theme(), "blake3:themehash").unwrap(), 0);
    }

    #[test]
    fn untokened_values_are_never_touched() {
        let mut doc = crate::fixture::lower_third();
        let before = doc.root.children[0].style.fill.clone();
        resolve(&mut doc, &theme(), "blake3:t").unwrap();
        assert_eq!(doc.root.children[0].style.fill, before);
    }

    #[test]
    fn a_dangling_token_is_an_error_not_a_fallback() {
        let mut doc = crate::fixture::lower_third();
        doc.root.children[0].style.fill = Some(Rgba {
            rgba: [0.0; 4],
            token: Some("brand.ghost".into()),
        });
        let e = resolve(&mut doc, &theme(), "blake3:t").unwrap_err();
        assert!(e.to_string().contains("brand.ghost"), "{e}");
    }

    #[test]
    fn a_font_token_moves_family_and_weight_together() {
        let mut doc = crate::fixture::lower_third();
        if let Some(Content::Text { font, .. }) = &mut doc.root.children[0].children[0].content {
            font.token = Some("brand.font".into());
            font.family = "Stale".into();
        }
        resolve(&mut doc, &theme(), "blake3:t").unwrap();
        if let Some(Content::Text { font, .. }) = &doc.root.children[0].children[0].content {
            assert_eq!(font.family, "IBM Plex Sans");
            assert_eq!(font.weight, 600);
        } else {
            panic!("fixture shape changed");
        }
    }
}
