//! The golden corpus — spike S1's parity suite, at the level of **templates**.
//!
//! `parity.rs` holds three hand-drawn primitives which between them cover what
//! a GPU does differently: fill, our own antialiasing math, and texture
//! sampling. They gated M0 and they still run. What they do not cover is
//! everything built on top of them since — layout, transforms, strokes,
//! shadows, gradients, clipping, images, animation, and text with its
//! CPU-rasterized effects. A shader edit that moved a gradient by a pixel would
//! have passed a three-figure suite without comment.
//!
//! So this module is the corpus M5's bar asks for: real `SceneDoc`s, armed and
//! drawn through the **same path a node runs**, one per feature interaction
//! worth pinning.
//!
//! # Why the documents are code and not files
//!
//! The browser half of the harness must build byte-identical documents to the
//! native half. Fifty JSON files fetched over HTTP would make "did both sides
//! render the same template?" a question about the web server, and a corpus
//! whose two halves can drift is not a parity instrument. Built here, in the
//! pure set, both targets compile the same constructors — which is ADR 0001's
//! premise applied to the thing that tests ADR 0001.
//!
//! # Why the rendered images are not committed
//!
//! A committed baseline is only meaningful against a reproducible input, and a
//! GPU is not one: the three S1 PNGs in `testdata/golden/` are Metal artifacts
//! and a Linux runner would fail a byte compare against them for reasons that
//! are not bugs. The corpus is therefore compared:
//!
//! - **native against native**, twice in a row — determinism, on every CI run;
//! - **native against WASM**, both handed the same font bytes — the S1 bar,
//!   run by hand and recorded in `docs/INTEGRATION-NOTES.md`;
//! - **before against after**, locally, across a shader or wgpu change — the
//!   ritual in `docs/TESTING.md`, which this corpus is what gives teeth to.
//!
//! # Fonts
//!
//! `pmg-text` takes font BYTES and has no system font database, deliberately.
//! The corpus inherits that: text figures render only when the harness is
//! handed a face, and are **skipped loudly** otherwise rather than quietly
//! shrinking the suite. Both sides must be given the same bytes; both print the
//! face's length and hash so a mismatch is visible rather than mysterious.

use pmg_scene::bind::Data;
use pmg_scene::doc::{
    Align, Binding, Content, Dimension, Direction, Edges, Field, FieldType, Fit, FontRef, FontSpec,
    Gradient, GradientStop, Justify, Layout, Node, NodeKind, Position, Rgba, SceneDoc, Shadow,
    Stroke, Style, Timeline, Track, Transform2D, VisibleIf, VisibleOp,
};
use std::collections::BTreeMap;
use std::sync::Arc;

/// Corpus canvas, in design units. 16:9 because every real template is, and
/// 512 wide because `512 * 4` is already a multiple of wgpu's 256-byte readback
/// row alignment — no padding path to get subtly wrong inside the instrument.
pub const CANVAS_W: u32 = 512;
pub const CANVAS_H: u32 = 288;

// Authored colours are linear-light straight RGBA (ADR 0012). Values chosen to
// be unremarkable: the corpus is measured, not looked at.
const RED: [f32; 4] = [0.72, 0.11, 0.15, 1.0];
const BLUE: [f32; 4] = [0.11, 0.30, 0.85, 1.0];
const GREEN: [f32; 4] = [0.13, 0.62, 0.35, 1.0];
const AMBER: [f32; 4] = [0.95, 0.68, 0.13, 1.0];
const TEAL: [f32; 4] = [0.09, 0.45, 0.48, 1.0];
const CREAM: [f32; 4] = [0.96, 0.94, 0.86, 1.0];
const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
const BLACK: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
const HALF_BLUE: [f32; 4] = [0.11, 0.30, 0.85, 0.5];

/// One corpus entry: a document, plus what it is here to catch.
#[derive(Debug, Clone)]
pub struct Template {
    /// File-safe, stable. Renaming one orphans its history in every recorded
    /// run, so treat it the way node ids are treated.
    pub name: &'static str,
    /// What this figure would catch if it broke. Printed by the harness, so a
    /// failure arrives with its own explanation.
    pub covers: &'static str,
    /// Text figures need a face; see the module note.
    pub needs_font: bool,
    /// Where to evaluate the document's timelines. Non-zero only for the
    /// animation figures — mid-tween is where interpolation is worth pinning,
    /// because both ends are just the static frames either side of it.
    pub t_ms: f32,
    pub doc: SceneDoc,
}

// ---------------------------------------------------------------------------
// Construction helpers. Terse on purpose: seventy figures read as a table only
// if each one is a line or two.
// ---------------------------------------------------------------------------

fn node(id: &str, kind: NodeKind) -> Node {
    Node {
        id: id.into(),
        display_name: None,
        kind,
        layout: Layout::default(),
        transform: Transform2D::default(),
        style: Style::default(),
        clip: false,
        visible_if: None,
        content: None,
        children: vec![],
        unknown: Default::default(),
    }
}

fn px(w: f32, h: f32) -> Layout {
    Layout {
        width: Dimension::Px(w),
        height: Dimension::Px(h),
        ..Default::default()
    }
}

/// A fixed-size box at a margin offset — absolute placement without leaning on
/// flexbox, so a figure about strokes is not also a figure about layout.
fn at(id: &str, x: f32, y: f32, w: f32, h: f32, style: Style) -> Node {
    Node {
        layout: Layout {
            position: Position::Absolute,
            margin: Edges {
                top: y,
                left: x,
                ..Default::default()
            },
            ..px(w, h)
        },
        style,
        ..node(id, NodeKind::Box)
    }
}

fn fill(c: [f32; 4]) -> Style {
    Style {
        fill: Some(Rgba::flat(c)),
        ..Default::default()
    }
}

fn stops(v: &[([f32; 4], f32)]) -> Vec<GradientStop> {
    v.iter()
        .map(|(c, a)| GradientStop {
            color: Rgba::flat(*c),
            at: *a,
        })
        .collect()
}

fn grad(angle: f32, v: &[([f32; 4], f32)]) -> Style {
    Style {
        // `fill` stays equal to the first stop: that is what a 1.0 reader draws,
        // and the format asks writers to keep it so (doc.rs, Style::gradient).
        fill: Some(Rgba::flat(v[0].0)),
        gradient: Some(Gradient {
            angle_deg: angle,
            stops: stops(v),
        }),
        ..Default::default()
    }
}

fn doc_of(name: &'static str, children: Vec<Node>) -> SceneDoc {
    let mut d = SceneDoc::blank(&format!("corpus-{name}"), name, CANVAS_W, CANVAS_H);
    d.root.children = children;
    d
}

/// The family every text figure names. Registered by the harness against
/// whatever face it was given.
pub const FONT_FAMILY: &str = "Corpus";

fn font_doc(name: &'static str, children: Vec<Node>) -> SceneDoc {
    let mut d = doc_of(name, children);
    d.fonts = vec![FontRef {
        family: FONT_FAMILY.into(),
        weight: 400,
        style: None,
        asset: None,
        system_fallback: false,
    }];
    d
}

fn spec(size: f32) -> FontSpec {
    FontSpec {
        family: FONT_FAMILY.into(),
        weight: 400,
        size,
        features: vec![],
        tracking: 0.0,
        line_height: 1.2,
        token: None,
    }
}

fn text_node(id: &str, expr: &str, font: FontSpec, align: Align) -> Node {
    Node {
        content: Some(Content::Text {
            binding: Binding {
                expr: expr.into(),
                transform: vec![],
            },
            font,
            align,
            max_lines: None,
        }),
        style: fill(WHITE),
        ..node(id, NodeKind::Text)
    }
}

fn field(key: &str, sample: &str) -> Field {
    Field {
        key: key.into(),
        ty: FieldType::Text,
        label: key.into(),
        required: false,
        max_len: None,
        sample: Some(sample.into()),
    }
}

fn t(name: &'static str, covers: &'static str, doc: SceneDoc) -> Template {
    Template {
        name,
        covers,
        needs_font: !doc.fonts.is_empty(),
        t_ms: 0.0,
        doc,
    }
}

impl Template {
    /// Evaluate this figure's timelines at `ms` rather than at zero.
    fn at_ms(mut self, ms: f32) -> Self {
        self.t_ms = ms;
        self
    }
}

/// A track of two keys — the shape every animation figure uses.
fn track(target: &str, from: f32, to: f32, dur: f32, ease: pmg_anim::Ease) -> Track {
    Track {
        target: target.into(),
        blend: Default::default(),
        keys: vec![
            pmg_anim::Key {
                t: 0.0,
                v: from,
                ease,
            },
            pmg_anim::Key {
                t: dur,
                v: to,
                ease: pmg_anim::Ease::Linear,
            },
        ],
    }
}

fn timeline(tracks: Vec<Track>, dur: f32) -> Timeline {
    Timeline {
        name: "In".into(),
        duration_ms: dur,
        on_complete: Default::default(),
        looping: false,
        tracks,
    }
}

// ---------------------------------------------------------------------------
// The test image
// ---------------------------------------------------------------------------

/// The corpus's one picture: procedural, so both targets hold identical bytes
/// with no fetch and no decoder in the measurement.
///
/// Deliberately 3:2 — an aspect that matches neither the canvas nor any box it
/// is placed in, so `Cover`, `Contain`, and `Stretch` each produce a visibly
/// different result rather than three copies of the same figure.
pub fn test_image() -> (u32, u32, Vec<u8>) {
    let (w, h) = (96u32, 64u32);
    let mut px = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            // A checker so any flip or transpose is obvious, over a ramp so a
            // sampling difference shows as a shift rather than as nothing.
            let checker = ((x / 8) + (y / 8)) % 2 == 0;
            let ramp = (x * 255 / (w - 1)) as u8;
            px[i] = if checker { ramp } else { 255 - ramp };
            px[i + 1] = (y * 255 / (h - 1)) as u8;
            px[i + 2] = if checker { 200 } else { 40 };
            px[i + 3] = 255;
        }
    }
    (w, h, px)
}

/// The corpus asset key every image figure names.
pub const IMAGE_KEY: &str = "picture";

fn image_node(id: &str, fit: Fit, w: f32, h: f32) -> Node {
    Node {
        layout: Layout {
            position: Position::Absolute,
            margin: Edges {
                top: 44.0,
                left: 96.0,
                ..Default::default()
            },
            ..px(w, h)
        },
        content: Some(Content::Image {
            asset_key: IMAGE_KEY.into(),
            fit,
            field: None,
        }),
        ..node(id, NodeKind::Image)
    }
}

fn image_doc(name: &'static str, children: Vec<Node>) -> SceneDoc {
    let mut d = doc_of(name, children);
    d.assets = vec![pmg_scene::doc::AssetRef {
        key: IMAGE_KEY.into(),
        // The harness supplies the pixels; the hash is the corpus's own, and is
        // never fetched from anywhere.
        hash: "blake3:0000000000000000000000000000000000000000000000000000000000000000".into(),
        kind: "image".into(),
    }];
    d
}

// ---------------------------------------------------------------------------
// The corpus
// ---------------------------------------------------------------------------

/// Every figure, in a stable order.
// Pushed rather than one `vec![]` literal: the figures are grouped by what they
// cover, with commentary between the groups, and a single expression spanning a
// thousand lines would put every group's rationale inside it.
#[allow(clippy::vec_init_then_push)]
pub fn all() -> Vec<Template> {
    let mut v: Vec<Template> = Vec::new();

    // --- Fills and the box itself -----------------------------------------
    v.push(t(
        "box-solid",
        "a flat fill — the frame everything else is a variation on",
        doc_of(
            "box-solid",
            vec![at("a", 96.0, 64.0, 320.0, 160.0, fill(RED))],
        ),
    ));
    v.push(t(
        "box-radius",
        "the rounded-rect SDF at a moderate radius",
        doc_of(
            "box-radius",
            vec![at(
                "a",
                96.0,
                64.0,
                320.0,
                160.0,
                Style {
                    radius: 24.0,
                    ..fill(RED)
                },
            )],
        ),
    ));
    v.push(t(
        "box-radius-pill",
        "radius clamped to half the short side — the corner case in the SDF",
        doc_of(
            "box-radius-pill",
            vec![at(
                "a",
                96.0,
                104.0,
                320.0,
                80.0,
                Style {
                    radius: 40.0,
                    ..fill(TEAL)
                },
            )],
        ),
    ));
    v.push(t(
        "box-opacity",
        "node opacity, applied in linear light before encode",
        doc_of(
            "box-opacity",
            vec![at(
                "a",
                96.0,
                64.0,
                320.0,
                160.0,
                Style {
                    opacity: 0.4,
                    ..fill(RED)
                },
            )],
        ),
    ));
    v.push(t(
        "box-alpha-fill",
        "a translucent fill over an opaque one — premultiplied compositing",
        doc_of(
            "box-alpha-fill",
            vec![
                at("under", 64.0, 64.0, 240.0, 160.0, fill(AMBER)),
                at("over", 176.0, 104.0, 240.0, 120.0, fill(HALF_BLUE)),
            ],
        ),
    ));
    v.push(t(
        "box-overlap",
        "draw order — later siblings land on top",
        doc_of(
            "box-overlap",
            vec![
                at("a", 64.0, 48.0, 200.0, 140.0, fill(RED)),
                at("b", 140.0, 88.0, 200.0, 140.0, fill(GREEN)),
                at("c", 216.0, 128.0, 200.0, 140.0, fill(BLUE)),
            ],
        ),
    ));
    v.push(t(
        "box-nested",
        "nested boxes with padding — parent rect drives the child's",
        doc_of(
            "box-nested",
            vec![Node {
                layout: Layout {
                    padding: Edges {
                        top: 24.0,
                        right: 24.0,
                        bottom: 24.0,
                        left: 24.0,
                    },
                    ..px(320.0, 200.0)
                },
                style: fill(TEAL),
                children: vec![Node {
                    layout: Layout {
                        padding: Edges {
                            top: 20.0,
                            right: 20.0,
                            bottom: 20.0,
                            left: 20.0,
                        },
                        width: Dimension::Percent(100.0),
                        height: Dimension::Percent(100.0),
                        ..Default::default()
                    },
                    style: fill(CREAM),
                    children: vec![Node {
                        layout: Layout {
                            width: Dimension::Percent(100.0),
                            height: Dimension::Percent(100.0),
                            ..Default::default()
                        },
                        style: fill(RED),
                        ..node("inner", NodeKind::Box)
                    }],
                    ..node("mid", NodeKind::Box)
                }],
                ..node("outer", NodeKind::Box)
            }],
        ),
    ));
    v.push(t(
        "box-hairline",
        "a one-unit-tall box — where an off-by-half in the SDF shows first",
        doc_of(
            "box-hairline",
            vec![
                at("h", 96.0, 143.0, 320.0, 1.0, fill(WHITE)),
                at("v", 255.0, 64.0, 1.0, 160.0, fill(WHITE)),
            ],
        ),
    ));

    // --- Strokes -----------------------------------------------------------
    let stroked = |w: f32, radius: f32, color: [f32; 4], fill_c: Option<[f32; 4]>| Style {
        fill: fill_c.map(Rgba::flat),
        stroke: Some(Stroke {
            color: Rgba::flat(color),
            width: w,
        }),
        radius,
        ..Default::default()
    };
    v.push(t(
        "stroke-hairline",
        "a one-unit stroke, inside the edge",
        doc_of(
            "stroke-hairline",
            vec![at(
                "a",
                96.0,
                64.0,
                320.0,
                160.0,
                stroked(1.0, 0.0, WHITE, Some(TEAL)),
            )],
        ),
    ));
    v.push(t(
        "stroke-thick",
        "a wide stroke — the inside-the-edge rule, visibly",
        doc_of(
            "stroke-thick",
            vec![at(
                "a",
                96.0,
                64.0,
                320.0,
                160.0,
                stroked(16.0, 0.0, AMBER, Some(TEAL)),
            )],
        ),
    ));
    v.push(t(
        "stroke-radius",
        "stroke following a rounded silhouette",
        doc_of(
            "stroke-radius",
            vec![at(
                "a",
                96.0,
                64.0,
                320.0,
                160.0,
                stroked(8.0, 28.0, AMBER, Some(TEAL)),
            )],
        ),
    ));
    v.push(t(
        "stroke-only",
        "an outline with no fill — the stroke's inner AA edge alone",
        doc_of(
            "stroke-only",
            vec![at(
                "a",
                96.0,
                64.0,
                320.0,
                160.0,
                stroked(6.0, 12.0, WHITE, None),
            )],
        ),
    ));
    v.push(t(
        "stroke-alpha",
        "a translucent stroke over its own fill",
        doc_of(
            "stroke-alpha",
            vec![at(
                "a",
                96.0,
                64.0,
                320.0,
                160.0,
                stroked(20.0, 0.0, [1.0, 1.0, 1.0, 0.45], Some(RED)),
            )],
        ),
    ));
    v.push(t(
        "stroke-on-gradient",
        "stroke and gradient on one node — two fill paths in one draw",
        doc_of(
            "stroke-on-gradient",
            vec![at(
                "a",
                96.0,
                64.0,
                320.0,
                160.0,
                Style {
                    stroke: Some(Stroke {
                        color: Rgba::flat(WHITE),
                        width: 10.0,
                    }),
                    radius: 20.0,
                    ..grad(90.0, &[(RED, 0.0), (BLUE, 1.0)])
                },
            )],
        ),
    ));

    // --- Shadows -----------------------------------------------------------
    let shadowed = |dx: f32, dy: f32, blur: f32, radius: f32, c: [f32; 4], f: [f32; 4]| Style {
        shadow: Some(Shadow {
            color: Rgba::flat(c),
            dx,
            dy,
            blur,
        }),
        radius,
        ..fill(f)
    };
    v.push(t(
        "shadow-hard",
        "blur 0 — a hard offset copy of the silhouette",
        doc_of(
            "shadow-hard",
            vec![at(
                "a",
                96.0,
                64.0,
                300.0,
                150.0,
                shadowed(16.0, 16.0, 0.0, 0.0, BLACK, CREAM),
            )],
        ),
    ));
    v.push(t(
        "shadow-soft",
        "a blurred lobe — the falloff every soft edge downstream depends on",
        doc_of(
            "shadow-soft",
            vec![at(
                "a",
                96.0,
                64.0,
                300.0,
                150.0,
                shadowed(12.0, 14.0, 18.0, 0.0, BLACK, CREAM),
            )],
        ),
    ));
    v.push(t(
        "shadow-negative-offset",
        "a shadow up and to the left — sign handling in the offset",
        doc_of(
            "shadow-negative-offset",
            vec![at(
                "a",
                106.0,
                74.0,
                300.0,
                150.0,
                shadowed(-14.0, -12.0, 10.0, 0.0, BLACK, CREAM),
            )],
        ),
    ));
    v.push(t(
        "shadow-radius",
        "a rounded shadow — the SDF drives the lobe, not a rectangle",
        doc_of(
            "shadow-radius",
            vec![at(
                "a",
                96.0,
                64.0,
                300.0,
                150.0,
                shadowed(14.0, 14.0, 14.0, 32.0, BLACK, CREAM),
            )],
        ),
    ));
    v.push(t(
        "shadow-knockout",
        "a translucent box over its own shadow — the knockout, which is the \
         difference between a box shadow and a text shadow",
        doc_of(
            "shadow-knockout",
            vec![at(
                "a",
                96.0,
                64.0,
                300.0,
                150.0,
                Style {
                    fill: Some(Rgba::flat([0.96, 0.94, 0.86, 0.45])),
                    ..shadowed(18.0, 18.0, 12.0, 0.0, BLACK, CREAM)
                },
            )],
        ),
    ));
    v.push(t(
        "shadow-and-stroke",
        "shadow, stroke and fill on one node — the whole decoration order",
        doc_of(
            "shadow-and-stroke",
            vec![at(
                "a",
                96.0,
                64.0,
                300.0,
                150.0,
                Style {
                    stroke: Some(Stroke {
                        color: Rgba::flat(AMBER),
                        width: 8.0,
                    }),
                    ..shadowed(12.0, 12.0, 10.0, 20.0, BLACK, TEAL)
                },
            )],
        ),
    ));

    // --- Gradients ---------------------------------------------------------
    for (name, angle) in [
        ("gradient-up", 0.0f32),
        ("gradient-right", 90.0),
        ("gradient-down", 180.0),
        ("gradient-left", 270.0),
        ("gradient-diagonal", 45.0),
        ("gradient-anti-diagonal", 135.0),
    ] {
        let covers: &'static str = match name {
            "gradient-up" => "the CSS angle dial at 0° — up, and the axis sign that goes with it",
            "gradient-right" => "90° — left to right, the common case",
            "gradient-down" => "180° — top to bottom",
            "gradient-left" => "270° — the wrap side of the dial",
            "gradient-diagonal" => "45° — both axes at once, where a snapped direction shows",
            _ => "135° — the other diagonal, catching a transposed axis",
        };
        v.push(t(
            name,
            covers,
            doc_of(
                name,
                vec![at(
                    "a",
                    96.0,
                    64.0,
                    320.0,
                    160.0,
                    grad(angle, &[(RED, 0.0), (BLUE, 1.0)]),
                )],
            ),
        ));
    }
    v.push(t(
        "gradient-three-stop",
        "a mid stop — the sequential mix walk, not a single lerp",
        doc_of(
            "gradient-three-stop",
            vec![at(
                "a",
                96.0,
                64.0,
                320.0,
                160.0,
                grad(90.0, &[(RED, 0.0), (AMBER, 0.5), (BLUE, 1.0)]),
            )],
        ),
    ));
    v.push(t(
        "gradient-four-stop",
        "the format's maximum stop count, unevenly spaced",
        doc_of(
            "gradient-four-stop",
            vec![at(
                "a",
                96.0,
                64.0,
                320.0,
                160.0,
                grad(
                    120.0,
                    &[(RED, 0.0), (AMBER, 0.2), (GREEN, 0.75), (BLUE, 1.0)],
                ),
            )],
        ),
    ));
    v.push(t(
        "gradient-radius",
        "a gradient inside a rounded silhouette — fill path and SDF together",
        doc_of(
            "gradient-radius",
            vec![at(
                "a",
                96.0,
                64.0,
                320.0,
                160.0,
                Style {
                    radius: 36.0,
                    ..grad(45.0, &[(TEAL, 0.0), (CREAM, 1.0)])
                },
            )],
        ),
    ));
    v.push(t(
        "gradient-alpha",
        "a stop that fades to transparent — interpolation of premultiplied alpha",
        doc_of(
            "gradient-alpha",
            vec![
                at("under", 64.0, 48.0, 384.0, 192.0, fill(CREAM)),
                at(
                    "a",
                    96.0,
                    64.0,
                    320.0,
                    160.0,
                    grad(90.0, &[(BLUE, 0.0), ([0.11, 0.30, 0.85, 0.0], 1.0)]),
                ),
            ],
        ),
    ));
    v.push(t(
        "gradient-clustered-stops",
        "two stops nearly on top of each other — a hard band, where a walk that \
         divides by the span would divide by zero",
        doc_of(
            "gradient-clustered-stops",
            vec![at(
                "a",
                96.0,
                64.0,
                320.0,
                160.0,
                grad(90.0, &[(RED, 0.0), (RED, 0.499), (BLUE, 0.5), (BLUE, 1.0)]),
            )],
        ),
    ));

    // --- Transforms --------------------------------------------------------
    let xf = |tr: Transform2D| Node {
        transform: tr,
        ..at("a", 128.0, 84.0, 256.0, 120.0, fill(RED))
    };
    v.push(t(
        "transform-translate",
        "a translated node — the affine's simplest term",
        doc_of(
            "transform-translate",
            vec![xf(Transform2D {
                translate: [40.0, -24.0],
                ..Default::default()
            })],
        ),
    ));
    v.push(t(
        "transform-rotate",
        "rotation about the centre anchor — AA on a non-axis-aligned edge",
        doc_of(
            "transform-rotate",
            vec![xf(Transform2D {
                rotate: 15.0,
                ..Default::default()
            })],
        ),
    ));
    v.push(t(
        "transform-rotate-quarter",
        "exactly 90° — the case where a sin/cos last-bit difference would show \
         as a visibly crooked edge rather than as noise",
        doc_of(
            "transform-rotate-quarter",
            vec![xf(Transform2D {
                rotate: 90.0,
                ..Default::default()
            })],
        ),
    ));
    v.push(t(
        "transform-scale",
        "uniform scale about the centre",
        doc_of(
            "transform-scale",
            vec![xf(Transform2D {
                scale: [1.4, 1.4],
                ..Default::default()
            })],
        ),
    ));
    v.push(t(
        "transform-scale-nonuniform",
        "different scale per axis — a transposed matrix looks right until this",
        doc_of(
            "transform-scale-nonuniform",
            vec![xf(Transform2D {
                scale: [1.6, 0.6],
                ..Default::default()
            })],
        ),
    ));
    v.push(t(
        "transform-anchor-corner",
        "the same rotation about a corner anchor — anchor applied before rotate",
        doc_of(
            "transform-anchor-corner",
            vec![xf(Transform2D {
                rotate: 15.0,
                anchor: [0.0, 0.0],
                ..Default::default()
            })],
        ),
    ));
    v.push(t(
        "transform-skew-ignored",
        "skew is in the FORMAT and this reader does not draw it — the figure \
         renders as an unskewed box on purpose, so the day skew is implemented \
         this image changes and somebody has to look at it",
        doc_of(
            "transform-skew-ignored",
            vec![xf(Transform2D {
                skew: [12.0, 6.0],
                ..Default::default()
            })],
        ),
    ));
    v.push(t(
        "transform-nested",
        "a child transform composed with its parent's — the order that makes a \
         reveal animate about the band rather than about the screen",
        doc_of(
            "transform-nested",
            vec![Node {
                transform: Transform2D {
                    rotate: 12.0,
                    ..Default::default()
                },
                children: vec![Node {
                    transform: Transform2D {
                        translate: [30.0, 20.0],
                        scale: [0.6, 0.6],
                        ..Default::default()
                    },
                    ..at("child", 20.0, 20.0, 160.0, 100.0, fill(AMBER))
                }],
                ..at("parent", 128.0, 84.0, 256.0, 140.0, fill(TEAL))
            }],
        ),
    ));

    // --- Layout ------------------------------------------------------------
    let row_child = |id: &str, w: f32, c: [f32; 4]| Node {
        layout: px(w, 80.0),
        style: fill(c),
        ..node(id, NodeKind::Box)
    };
    let flex = |name: &'static str, layout: Layout, kids: Vec<Node>| {
        doc_of(
            name,
            vec![Node {
                layout,
                style: fill([0.10, 0.12, 0.16, 1.0]),
                children: kids,
                ..node("bar", NodeKind::Box)
            }],
        )
    };
    let base_row = Layout {
        direction: Direction::Row,
        align: Align::Center,
        padding: Edges {
            top: 16.0,
            right: 16.0,
            bottom: 16.0,
            left: 16.0,
        },
        ..px(CANVAS_W as f32, 160.0)
    };
    v.push(t(
        "layout-row-gap",
        "a row with a gap — the flexbox main axis",
        flex(
            "layout-row-gap",
            Layout {
                gap: 24.0,
                ..base_row
            },
            vec![
                row_child("a", 100.0, RED),
                row_child("b", 100.0, GREEN),
                row_child("c", 100.0, BLUE),
            ],
        ),
    ));
    v.push(t(
        "layout-column-gap",
        "a column with a gap — the cross axis of the same solver",
        flex(
            "layout-column-gap",
            Layout {
                direction: Direction::Column,
                gap: 12.0,
                align: Align::Start,
                ..px(CANVAS_W as f32, 240.0)
            },
            vec![
                row_child("a", 200.0, RED),
                row_child("b", 260.0, GREEN),
                row_child("c", 140.0, BLUE),
            ],
        ),
    ));
    v.push(t(
        "layout-justify-center",
        "justify: centre — free space split evenly at both ends",
        flex(
            "layout-justify-center",
            Layout {
                justify: Justify::Center,
                gap: 16.0,
                ..base_row
            },
            vec![row_child("a", 100.0, RED), row_child("b", 100.0, BLUE)],
        ),
    ));
    v.push(t(
        "layout-justify-between",
        "space-between — free space to the gaps, none to the ends",
        flex(
            "layout-justify-between",
            Layout {
                justify: Justify::SpaceBetween,
                ..base_row
            },
            vec![
                row_child("a", 90.0, RED),
                row_child("b", 90.0, GREEN),
                row_child("c", 90.0, BLUE),
            ],
        ),
    ));
    v.push(t(
        "layout-align-stretch",
        "align: stretch against fixed-height siblings — the default that \
         surprises people",
        flex(
            "layout-align-stretch",
            Layout {
                align: Align::Stretch,
                gap: 20.0,
                ..base_row
            },
            vec![
                Node {
                    layout: Layout {
                        width: Dimension::Px(120.0),
                        height: Dimension::Auto,
                        ..Default::default()
                    },
                    style: fill(RED),
                    ..node("a", NodeKind::Box)
                },
                row_child("b", 120.0, GREEN),
            ],
        ),
    ));
    v.push(t(
        "layout-grow-shrink",
        "grow taking the free space — the band that fills its row",
        flex(
            "layout-grow-shrink",
            Layout {
                gap: 16.0,
                ..base_row
            },
            vec![
                row_child("a", 80.0, RED),
                Node {
                    layout: Layout {
                        grow: 1.0,
                        height: Dimension::Px(80.0),
                        ..Default::default()
                    },
                    style: fill(GREEN),
                    ..node("b", NodeKind::Box)
                },
                row_child("c", 80.0, BLUE),
            ],
        ),
    ));
    v.push(t(
        "layout-percent",
        "percentage dimensions against a fixed parent",
        flex(
            "layout-percent",
            Layout {
                gap: 8.0,
                ..base_row
            },
            vec![
                Node {
                    layout: Layout {
                        width: Dimension::Percent(25.0),
                        height: Dimension::Percent(60.0),
                        ..Default::default()
                    },
                    style: fill(RED),
                    ..node("a", NodeKind::Box)
                },
                Node {
                    layout: Layout {
                        width: Dimension::Percent(50.0),
                        height: Dimension::Percent(90.0),
                        ..Default::default()
                    },
                    style: fill(BLUE),
                    ..node("b", NodeKind::Box)
                },
            ],
        ),
    ));
    v.push(t(
        "layout-margin-padding",
        "margins and padding on the same node — the two that get confused",
        flex(
            "layout-margin-padding",
            Layout {
                padding: Edges {
                    top: 24.0,
                    right: 48.0,
                    bottom: 24.0,
                    left: 12.0,
                },
                gap: 8.0,
                ..base_row
            },
            vec![
                Node {
                    layout: Layout {
                        margin: Edges {
                            top: 10.0,
                            right: 0.0,
                            bottom: 0.0,
                            left: 30.0,
                        },
                        ..px(120.0, 70.0)
                    },
                    style: fill(RED),
                    ..node("a", NodeKind::Box)
                },
                row_child("b", 120.0, BLUE),
            ],
        ),
    ));

    // --- Clipping ----------------------------------------------------------
    v.push(t(
        "clip-overflow",
        "a child larger than its clipping parent — the scissor, in design units \
         against a target of a different size",
        doc_of(
            "clip-overflow",
            // The child overflows on three sides and stops short on the
            // fourth, so the frame's own fill stays visible: a figure where the
            // child covers everything cannot tell a working clip from a
            // coincidence.
            vec![Node {
                clip: true,
                children: vec![at("child", -40.0, -30.0, 240.0, 260.0, fill(AMBER))],
                ..at("frame", 128.0, 74.0, 256.0, 140.0, fill(TEAL))
            }],
        ),
    ));
    v.push(t(
        "clip-rounded-frame",
        "clip on a rounded parent — the scissor is rectangular and the fill is \
         not, which is the honest current behaviour and worth pinning",
        doc_of(
            "clip-rounded-frame",
            vec![Node {
                clip: true,
                children: vec![at("child", -20.0, -20.0, 400.0, 260.0, fill(AMBER))],
                ..at(
                    "frame",
                    128.0,
                    74.0,
                    256.0,
                    140.0,
                    Style {
                        radius: 36.0,
                        ..fill(TEAL)
                    },
                )
            }],
        ),
    ));
    v.push(t(
        "clip-nested",
        "two clips in a chain — the inner rect must intersect the outer, not \
         replace it",
        doc_of(
            "clip-nested",
            vec![Node {
                clip: true,
                children: vec![Node {
                    clip: true,
                    children: vec![at("deep", -60.0, -60.0, 400.0, 300.0, fill(RED))],
                    ..at("inner", 80.0, 30.0, 200.0, 160.0, fill(CREAM))
                }],
                ..at("outer", 96.0, 64.0, 240.0, 120.0, fill(TEAL))
            }],
        ),
    ));
    v.push(t(
        "clip-with-transform",
        "a transformed child inside a clip — the rect is in target pixels and \
         the child is not",
        doc_of(
            "clip-with-transform",
            vec![Node {
                clip: true,
                children: vec![Node {
                    transform: Transform2D {
                        rotate: 20.0,
                        translate: [30.0, 10.0],
                        ..Default::default()
                    },
                    ..at("child", -20.0, -20.0, 320.0, 220.0, fill(AMBER))
                }],
                ..at("frame", 128.0, 74.0, 256.0, 140.0, fill(TEAL))
            }],
        ),
    ));

    // --- Images ------------------------------------------------------------
    v.push(t(
        "image-cover",
        "Cover — aspect preserved, overflow cropped",
        image_doc(
            "image-cover",
            vec![image_node("i", Fit::Cover, 320.0, 200.0)],
        ),
    ));
    v.push(t(
        "image-contain",
        "Contain — aspect preserved, gaps left. The box is deliberately far \
         from the picture's 3:2, so the letterbox is unmistakable rather than a \
         few pixels of nothing",
        image_doc(
            "image-contain",
            vec![image_node("i", Fit::Contain, 320.0, 120.0)],
        ),
    ));
    v.push(t(
        "image-stretch",
        "Stretch — aspect deliberately not preserved",
        image_doc(
            "image-stretch",
            vec![image_node("i", Fit::Stretch, 320.0, 200.0)],
        ),
    ));
    v.push(t(
        "image-tall-box",
        "the same picture in a box taller than it is wide — Cover cropping the \
         other axis",
        image_doc(
            "image-tall-box",
            vec![image_node("i", Fit::Cover, 140.0, 200.0)],
        ),
    ));
    v.push(t(
        "image-clipped-round",
        "an image inside a rounded, clipping parent — sampling and scissor \
         together",
        image_doc(
            "image-clipped-round",
            vec![Node {
                clip: true,
                children: vec![Node {
                    layout: Layout {
                        width: Dimension::Percent(100.0),
                        height: Dimension::Percent(100.0),
                        ..Default::default()
                    },
                    content: Some(Content::Image {
                        asset_key: IMAGE_KEY.into(),
                        fit: Fit::Cover,
                        field: None,
                    }),
                    ..node("i", NodeKind::Image)
                }],
                ..at(
                    "frame",
                    128.0,
                    64.0,
                    256.0,
                    160.0,
                    Style {
                        radius: 28.0,
                        ..fill(TEAL)
                    },
                )
            }],
        ),
    ));
    v.push(t(
        "image-under-box",
        "a translucent plate over a picture — the plate-over-video case",
        image_doc(
            "image-under-box",
            vec![
                image_node("i", Fit::Cover, 384.0, 216.0),
                at(
                    "plate",
                    96.0,
                    150.0,
                    320.0,
                    80.0,
                    Style {
                        fill: Some(Rgba::flat([0.0, 0.0, 0.0, 0.55])),
                        ..Default::default()
                    },
                ),
            ],
        ),
    ));

    // --- Animation, sampled mid-tween --------------------------------------
    let anim = |name: &'static str, tracks: Vec<Track>, dur: f32| {
        let mut d = doc_of(name, vec![at("a", 128.0, 84.0, 256.0, 120.0, fill(RED))]);
        d.timelines = vec![timeline(tracks, dur)];
        d
    };
    v.push(
        t(
            "anim-translate-mid",
            "a linear translate exactly half way — interpolation, not an endpoint",
            anim(
                "anim-translate-mid",
                vec![track(
                    "a/transform.translate.x",
                    -200.0,
                    0.0,
                    1000.0,
                    pmg_anim::Ease::Linear,
                )],
                1000.0,
            ),
        )
        .at_ms(500.0),
    );
    v.push(
        t(
            "anim-ease-out-mid",
            "the same travel under a cubic ease — the curve itself, in pixels",
            anim(
                "anim-ease-out-mid",
                vec![track(
                    "a/transform.translate.x",
                    -200.0,
                    0.0,
                    1000.0,
                    pmg_anim::Ease::Cubic([0.16, 1.0, 0.3, 1.0]),
                )],
                1000.0,
            ),
        )
        .at_ms(500.0),
    );
    v.push(
        t(
            "anim-steps-mid",
            "a stepped ease — quantised progress, which a lerp would smooth over",
            anim(
                "anim-steps-mid",
                vec![track(
                    "a/transform.translate.x",
                    -200.0,
                    0.0,
                    1000.0,
                    pmg_anim::Ease::Steps(4),
                )],
                1000.0,
            ),
        )
        .at_ms(620.0),
    );
    v.push(
        t(
            "anim-opacity-mid",
            "opacity animated to a fraction — the value the compositor sees",
            anim(
                "anim-opacity-mid",
                vec![track(
                    "a/style.opacity",
                    0.0,
                    1.0,
                    1000.0,
                    pmg_anim::Ease::Linear,
                )],
                1000.0,
            ),
        )
        .at_ms(400.0),
    );
    v.push(
        t(
            "anim-scale-mid",
            "scale mid-tween about the anchor",
            anim(
                "anim-scale-mid",
                vec![
                    track(
                        "a/transform.scale.x",
                        0.4,
                        1.0,
                        1000.0,
                        pmg_anim::Ease::Linear,
                    ),
                    track(
                        "a/transform.scale.y",
                        0.4,
                        1.0,
                        1000.0,
                        pmg_anim::Ease::Linear,
                    ),
                ],
                1000.0,
            ),
        )
        .at_ms(500.0),
    );
    v.push(
        t(
            "anim-multi-track",
            "four properties moving at once — the per-frame merge, not one track",
            anim(
                "anim-multi-track",
                vec![
                    track(
                        "a/transform.translate.x",
                        -120.0,
                        40.0,
                        1000.0,
                        pmg_anim::Ease::Linear,
                    ),
                    track(
                        "a/transform.translate.y",
                        60.0,
                        -20.0,
                        1000.0,
                        pmg_anim::Ease::Linear,
                    ),
                    track(
                        "a/transform.rotate",
                        -20.0,
                        10.0,
                        1000.0,
                        pmg_anim::Ease::Linear,
                    ),
                    track("a/style.opacity", 0.2, 1.0, 1000.0, pmg_anim::Ease::Linear),
                ],
                1000.0,
            ),
        )
        .at_ms(650.0),
    );
    v.push(
        t(
            "anim-before-first-key",
            "sampled before the first key — hold the first value, never zero",
            anim(
                "anim-before-first-key",
                vec![Track {
                    target: "a/transform.translate.x".into(),
                    blend: Default::default(),
                    keys: vec![
                        pmg_anim::Key {
                            t: 400.0,
                            v: -150.0,
                            ease: pmg_anim::Ease::Linear,
                        },
                        pmg_anim::Key {
                            t: 900.0,
                            v: 0.0,
                            ease: pmg_anim::Ease::Linear,
                        },
                    ],
                }],
                1000.0,
            ),
        )
        .at_ms(100.0),
    );

    // --- visible_if and the degraded path ----------------------------------
    let mut collapsed = doc_of(
        "visible-if-collapsed",
        vec![Node {
            layout: Layout {
                direction: Direction::Row,
                gap: 16.0,
                align: Align::Center,
                padding: Edges {
                    top: 16.0,
                    right: 16.0,
                    bottom: 16.0,
                    left: 16.0,
                },
                ..px(400.0, 120.0)
            },
            style: fill(TEAL),
            children: vec![
                Node {
                    layout: px(120.0, 80.0),
                    style: fill(CREAM),
                    ..node("kept", NodeKind::Box)
                },
                Node {
                    layout: px(120.0, 80.0),
                    style: fill(RED),
                    visible_if: Some(VisibleIf {
                        field: "title".into(),
                        op: VisibleOp::NonEmpty,
                    }),
                    ..node("dropped", NodeKind::Box)
                },
                Node {
                    layout: px(120.0, 80.0),
                    style: fill(AMBER),
                    ..node("after", NodeKind::Box)
                },
            ],
            ..at("bar", 56.0, 84.0, 400.0, 120.0, fill(TEAL))
        }],
    );
    // The sample is empty, so the middle child is REMOVED from layout and the
    // row closes up — the behaviour interpolation cannot express (§5.3).
    collapsed.fields = vec![Field {
        sample: Some(String::new()),
        ..field("title", "")
    }];
    v.push(t(
        "visible-if-collapsed",
        "a node removed from layout by visible_if — the row must close up, not \
         leave a hole",
        collapsed,
    ));

    let mut degraded = doc_of(
        "degraded-unknown-kind",
        vec![
            at("known", 64.0, 48.0, 160.0, 100.0, fill(TEAL)),
            Node {
                ..at("future", 248.0, 100.0, 180.0, 120.0, Style::default())
            },
        ],
    );
    degraded.root.children[1].kind = NodeKind::Other("hologram".into());
    v.push(t(
        "degraded-unknown-kind",
        "a node kind this reader does not know — a MAGENTA placeholder, because \
         rendering nothing and looking fine is the dangerous behaviour (§3.4)",
        degraded,
    ));

    // --- Masks (format 1.3) ------------------------------------------------
    //
    // A mask node masks its SIBLINGS. Every figure here is therefore a group
    // holding one mask plus the content it reveals, with an unmasked marker box
    // outside the group so a figure that masked everything away is still
    // distinguishable from a figure that failed to draw.
    fn mask_node(id: &str, x: f32, y: f32, w: f32, h: f32, radius: f32) -> Node {
        Node {
            kind: NodeKind::Mask,
            style: Style {
                radius,
                // No paint, ever: the compiler refuses it, because a reader too
                // old to know masks would draw that paint across the graphic.
                ..Default::default()
            },
            ..at(id, x, y, w, h, Style::default())
        }
    }
    let marker = || at("marker", 16.0, 16.0, 48.0, 24.0, fill(GREEN));
    let masked = |name: &'static str, mask: Node, kids: Vec<Node>| {
        let mut children = vec![mask];
        children.extend(kids);
        doc_of(
            name,
            vec![
                marker(),
                Node {
                    children,
                    ..at(
                        "group",
                        0.0,
                        0.0,
                        CANVAS_W as f32,
                        CANVAS_H as f32,
                        Style::default(),
                    )
                },
            ],
        )
    };

    v.push(t(
        "mask-reveals-part-of-a-box",
        "the mask in its simplest form — a rect revealing the middle of a box",
        masked(
            "mask-reveals-part-of-a-box",
            mask_node("m", 160.0, 96.0, 192.0, 96.0, 0.0),
            vec![at("a", 96.0, 64.0, 320.0, 160.0, fill(RED))],
        ),
    ));
    v.push(t(
        "mask-rounded",
        "a rounded mask — the same SDF the box path draws with, used as coverage",
        masked(
            "mask-rounded",
            mask_node("m", 160.0, 84.0, 192.0, 120.0, 44.0),
            vec![at("a", 96.0, 64.0, 320.0, 160.0, fill(RED))],
        ),
    ));
    v.push(t(
        "mask-edge-is-antialiased",
        "a mask edge at a half-unit offset — the coverage ramp, not a hard cut",
        masked(
            "mask-edge-is-antialiased",
            mask_node("m", 160.5, 96.5, 191.0, 95.0, 0.0),
            vec![at("a", 96.0, 64.0, 320.0, 160.0, fill(CREAM))],
        ),
    ));
    v.push(t(
        "mask-larger-than-content",
        "a mask that contains everything it masks — visually a no-op, and the \
         figure that would catch a mask being applied inside out",
        masked(
            "mask-larger-than-content",
            mask_node("m", 32.0, 24.0, 448.0, 240.0, 0.0),
            vec![at("a", 96.0, 64.0, 320.0, 160.0, fill(RED))],
        ),
    ));
    v.push(t(
        "mask-transformed",
        "a rotated mask over an unrotated box — mask and content have their own \
         transforms, which is the whole reason masking happens in world space. \
         The ONE figure in the corpus that is not bit-identical across targets: \
         2 pixels of 147 456 differ by 1/255 in one channel, on the angled mask \
         edge (2026-08-20, Metal). Inside the S1 bar and worth watching",
        masked(
            "mask-transformed",
            Node {
                transform: Transform2D {
                    rotate: 20.0,
                    ..Default::default()
                },
                ..mask_node("m", 128.0, 84.0, 256.0, 120.0, 0.0)
            },
            vec![at("a", 96.0, 64.0, 320.0, 160.0, fill(TEAL))],
        ),
    ));
    v.push(t(
        "mask-opacity",
        "a mask at 40% — masking multiplies coverage, so the content is a \
         translucent version of itself inside the shape and gone outside it",
        masked(
            "mask-opacity",
            Node {
                style: Style {
                    opacity: 0.4,
                    ..Default::default()
                },
                ..mask_node("m", 160.0, 96.0, 192.0, 96.0, 0.0)
            },
            vec![at("a", 96.0, 64.0, 320.0, 160.0, fill(RED))],
        ),
    ));
    v.push(t(
        "mask-over-image",
        "an image through a mask — every fragment path multiplies by the same \
         coverage, not just the box one",
        {
            let mut d = masked(
                "mask-over-image",
                mask_node("m", 128.0, 64.0, 224.0, 160.0, 40.0),
                vec![image_node("i", Fit::Cover, 320.0, 200.0)],
            );
            d.assets = vec![pmg_scene::doc::AssetRef {
                key: IMAGE_KEY.into(),
                hash: "blake3:0000000000000000000000000000000000000000000000000000000000000000"
                    .into(),
                kind: "image".into(),
            }];
            d
        },
    ));
    v.push(t(
        "mask-and-clip",
        "a clip and a mask on the same subtree — a rectangular scissor in target \
         pixels and a shaped coverage multiply, which are different mechanisms \
         and must compose",
        masked(
            "mask-and-clip",
            mask_node("m", 96.0, 64.0, 200.0, 160.0, 0.0),
            vec![Node {
                clip: true,
                children: vec![at("child", -20.0, -20.0, 400.0, 260.0, fill(AMBER))],
                ..at("frame", 160.0, 84.0, 224.0, 120.0, fill(TEAL))
            }],
        ),
    ));
    v.push(t(
        "mask-degenerate-hides-everything",
        "a mask scaled to nothing. It has no area, so it reveals NOTHING — a \
         wipe animating up from zero must start hidden, not start fully visible \
         because a matrix could not be inverted. The green marker outside the \
         masked group is what tells this apart from a figure that failed to draw",
        masked(
            "mask-degenerate-hides-everything",
            Node {
                transform: Transform2D {
                    scale: [0.0, 1.0],
                    ..Default::default()
                },
                ..mask_node("m", 160.0, 96.0, 192.0, 96.0, 0.0)
            },
            vec![at("a", 96.0, 64.0, 320.0, 160.0, fill(RED))],
        ),
    ));

    // The wipe reveal, which is the reason masks exist at all: the mask slides
    // and the content stays put, sampled mid-travel.
    let mut wipe = {
        let mut d = font_doc(
            "mask-wipe-mid",
            vec![
                marker(),
                Node {
                    children: vec![
                        mask_node("m", 56.0, 74.0, 400.0, 140.0, 0.0),
                        Node {
                            layout: Layout {
                                direction: Direction::Column,
                                justify: Justify::Center,
                                padding: Edges {
                                    top: 20.0,
                                    right: 24.0,
                                    bottom: 20.0,
                                    left: 24.0,
                                },
                                position: Position::Absolute,
                                margin: Edges {
                                    top: 74.0,
                                    left: 56.0,
                                    ..Default::default()
                                },
                                ..px(400.0, 140.0)
                            },
                            style: fill([0.08, 0.10, 0.14, 1.0]),
                            children: vec![text_node("line", "${line}", spec(34.0), Align::Start)],
                            ..node("plate", NodeKind::Box)
                        },
                    ],
                    ..at(
                        "group",
                        0.0,
                        0.0,
                        CANVAS_W as f32,
                        CANVAS_H as f32,
                        Style::default(),
                    )
                },
            ],
        );
        d.fields = vec![field("line", "Wiped on, wiped off")];
        d
    };
    wipe.timelines = vec![timeline(
        vec![track(
            "m/transform.translate.x",
            -400.0,
            0.0,
            1000.0,
            pmg_anim::Ease::Linear,
        )],
        1000.0,
    )];
    v.push(
        t(
            "mask-wipe-mid",
            "the reveal masks exist for — the MASK animates while the plate and \
             its type stay put, sampled half way across",
            wipe,
        )
        .at_ms(500.0),
    );

    // --- Themes ------------------------------------------------------------
    let mut themed = doc_of(
        "theme-tokens",
        vec![at(
            "a",
            96.0,
            64.0,
            320.0,
            160.0,
            Style {
                fill: Some(Rgba {
                    rgba: RED,
                    token: Some("brand.primary".into()),
                }),
                stroke: Some(Stroke {
                    color: Rgba {
                        rgba: CREAM,
                        token: Some("brand.paper".into()),
                    },
                    width: 8.0,
                }),
                radius: 20.0,
                ..Default::default()
            },
        )],
    );
    themed.theme =
        Some("blake3:1111111111111111111111111111111111111111111111111111111111111111".into());
    v.push(t(
        "theme-tokens",
        "token provenance on fill and stroke — the values beside the tokens are \
         what renders, and this figure must be pixel-identical to an untokened \
         one (ADR 0021)",
        themed,
    ));
    v.push(t(
        "theme-tokens-untokened-twin",
        "the same colours with no tokens — the control for theme-tokens",
        doc_of(
            "theme-tokens-untokened-twin",
            vec![at(
                "a",
                96.0,
                64.0,
                320.0,
                160.0,
                Style {
                    stroke: Some(Stroke {
                        color: Rgba::flat(CREAM),
                        width: 8.0,
                    }),
                    radius: 20.0,
                    ..fill(RED)
                },
            )],
        ),
    ));

    // --- Text --------------------------------------------------------------
    let text_doc = |name: &'static str, sample: &str, kids: Vec<Node>| {
        let mut d = font_doc(name, kids);
        d.fields = vec![field("line", sample)];
        d
    };
    // A dark plate 400x140 at a fixed spot. Text alignment is relative to the
    // widest LINE (text_source.rs), so the figures that exercise it are the
    // multi-line ones — a single line has nothing to align against.
    let plate = |kids: Vec<Node>| Node {
        layout: Layout {
            direction: Direction::Column,
            justify: Justify::Center,
            padding: Edges {
                top: 20.0,
                right: 24.0,
                bottom: 20.0,
                left: 24.0,
            },
            position: Position::Absolute,
            margin: Edges {
                top: 74.0,
                left: 56.0,
                ..Default::default()
            },
            ..px(400.0, 140.0)
        },
        style: fill([0.08, 0.10, 0.14, 1.0]),
        children: kids,
        ..node("plate", NodeKind::Box)
    };

    v.push(t(
        "text-plain",
        "one line of shaped text — the glyph atlas path end to end",
        text_doc(
            "text-plain",
            "Pastor Dan Whitfield",
            vec![plate(vec![text_node(
                "line",
                "${line}",
                spec(36.0),
                Align::Start,
            )])],
        ),
    ));
    v.push(t(
        "text-align-center",
        "centred lines — alignment is relative to the WIDEST LINE, so it is \
         two lines of different length that show it and a single line that \
         cannot",
        text_doc(
            "text-align-center",
            "Welcome home\nto the ten o'clock service",
            vec![plate(vec![text_node(
                "line",
                "${line}",
                spec(36.0),
                Align::Center,
            )])],
        ),
    ));
    v.push(t(
        "text-align-end",
        "right-aligned lines — the opposite rounding at each run origin",
        text_doc(
            "text-align-end",
            "Welcome home\nto the ten o'clock service",
            vec![plate(vec![text_node(
                "line",
                "${line}",
                spec(36.0),
                Align::End,
            )])],
        ),
    ));
    v.push(t(
        "text-wrapped",
        "greedy wrapping to the solved width — kerning must not shift when a \
         later word wraps",
        text_doc(
            "text-wrapped",
            "Grace and peace to you from God our Father and the Lord Jesus Christ",
            vec![plate(vec![text_node(
                "line",
                "${line}",
                spec(28.0),
                Align::Start,
            )])],
        ),
    ));
    v.push(t(
        "text-hard-break",
        "a newline the author chose — phrasing, not wrapping",
        text_doc(
            "text-hard-break",
            "We are here to witness\nto share and testify",
            vec![plate(vec![text_node(
                "line",
                "${line}",
                spec(30.0),
                Align::Center,
            )])],
        ),
    ));
    v.push(t(
        "text-large",
        "display size — where coverage gamma and stem darkening show most",
        text_doc(
            "text-large",
            "Advent",
            vec![plate(vec![text_node(
                "line",
                "${line}",
                spec(72.0),
                Align::Center,
            )])],
        ),
    ));
    v.push(t(
        "text-small",
        "credit size — the AA tail, where truncation used to shudder",
        text_doc(
            "text-small",
            "Recorded live at the 9am service",
            vec![plate(vec![text_node(
                "line",
                "${line}",
                spec(14.0),
                Align::Start,
            )])],
        ),
    ));
    v.push(t(
        "text-tracking",
        "positive tracking — advances accumulate, and fractionally",
        text_doc(
            "text-tracking",
            "SERMON SERIES",
            vec![plate(vec![text_node(
                "line",
                "${line}",
                FontSpec {
                    tracking: 6.0,
                    ..spec(30.0)
                },
                Align::Center,
            )])],
        ),
    ));
    v.push(t(
        "text-line-height",
        "a tight line height over two lines — baseline stepping",
        text_doc(
            "text-line-height",
            "First line here\nSecond line here",
            vec![plate(vec![text_node(
                "line",
                "${line}",
                FontSpec {
                    line_height: 0.95,
                    ..spec(30.0)
                },
                Align::Start,
            )])],
        ),
    ));
    v.push(t(
        "text-fractional-offset",
        "text nudged by a third of a pixel — the ⅛-px phase cache, which is \
         what stops sliding text shuddering (R3)",
        text_doc(
            "text-fractional-offset",
            "Sliding lower third",
            vec![Node {
                transform: Transform2D {
                    translate: [0.3333, 0.0],
                    ..Default::default()
                },
                ..plate(vec![text_node("line", "${line}", spec(36.0), Align::Start)])
            }],
        ),
    ));
    v.push(t(
        "text-outline",
        "an outlined glyph — the CPU dilation ring, sampled",
        text_doc(
            "text-outline",
            "Outlined",
            vec![plate(vec![Node {
                style: Style {
                    stroke: Some(Stroke {
                        color: Rgba::flat(RED),
                        width: 3.0,
                    }),
                    ..fill(WHITE)
                },
                ..text_node("line", "${line}", spec(56.0), Align::Center)
            }])],
        ),
    ));
    v.push(t(
        "text-shadow",
        "a text shadow — no knockout, unlike a box's (CSS text-shadow semantics)",
        text_doc(
            "text-shadow",
            "Shadowed",
            vec![plate(vec![Node {
                style: Style {
                    shadow: Some(Shadow {
                        color: Rgba::flat([0.0, 0.0, 0.0, 0.8]),
                        dx: 3.0,
                        dy: 4.0,
                        blur: 4.0,
                    }),
                    ..fill(WHITE)
                },
                ..text_node("line", "${line}", spec(56.0), Align::Center)
            }])],
        ),
    ));
    v.push(t(
        "text-outline-and-shadow",
        "both at once — shadow of the OUTLINED shape, then stroke, then fill",
        text_doc(
            "text-outline-and-shadow",
            "Both",
            vec![plate(vec![Node {
                style: Style {
                    stroke: Some(Stroke {
                        color: Rgba::flat(RED),
                        width: 3.0,
                    }),
                    shadow: Some(Shadow {
                        color: Rgba::flat([0.0, 0.0, 0.0, 0.8]),
                        dx: 4.0,
                        dy: 5.0,
                        blur: 6.0,
                    }),
                    ..fill(WHITE)
                },
                ..text_node("line", "${line}", spec(64.0), Align::Center)
            }])],
        ),
    ));
    v.push(t(
        "text-shadow-hard",
        "a text shadow with blur 0 — the hard-copy branch of the blur path",
        text_doc(
            "text-shadow-hard",
            "Hard shadow",
            vec![plate(vec![Node {
                style: Style {
                    shadow: Some(Shadow {
                        color: Rgba::flat(RED),
                        dx: 4.0,
                        dy: 4.0,
                        blur: 0.0,
                    }),
                    ..fill(WHITE)
                },
                ..text_node("line", "${line}", spec(48.0), Align::Center)
            }])],
        ),
    ));
    v.push(t(
        "text-on-gradient",
        "white type over a gradient plate — the contrast case linear \
         compositing was fixed for",
        text_doc(
            "text-on-gradient",
            "Light of the world",
            vec![Node {
                layout: Layout {
                    direction: Direction::Column,
                    justify: Justify::Center,
                    padding: Edges {
                        top: 20.0,
                        right: 24.0,
                        bottom: 20.0,
                        left: 24.0,
                    },
                    ..px(400.0, 140.0)
                },
                children: vec![text_node("line", "${line}", spec(34.0), Align::Center)],
                ..Node {
                    style: grad(90.0, &[(TEAL, 0.0), (BLUE, 1.0)]),
                    ..at("plate", 56.0, 74.0, 400.0, 140.0, Style::default())
                }
            }],
        ),
    ));
    v.push(t(
        "text-clipped",
        "text overflowing a clipping band — the pop-out reveal's mask",
        text_doc(
            "text-clipped",
            "This line is far too long for the band it is in",
            vec![Node {
                clip: true,
                layout: Layout {
                    direction: Direction::Column,
                    justify: Justify::Center,
                    padding: Edges {
                        top: 12.0,
                        right: 12.0,
                        bottom: 12.0,
                        left: 12.0,
                    },
                    ..px(240.0, 100.0)
                },
                children: vec![Node {
                    layout: Layout {
                        width: Dimension::Px(600.0),
                        ..Default::default()
                    },
                    ..text_node("line", "${line}", spec(30.0), Align::Start)
                }],
                ..at(
                    "band",
                    136.0,
                    94.0,
                    240.0,
                    100.0,
                    fill([0.08, 0.10, 0.14, 1.0]),
                )
            }],
        ),
    ));
    v.push(t(
        "text-fit-content-band",
        "a band hugging its text — Dimension::Content calling the measurer, \
         which IS the shrink-and-grow lower third",
        text_doc(
            "text-fit-content-band",
            "Kate Alvarez",
            vec![Node {
                layout: Layout {
                    direction: Direction::Row,
                    align: Align::Center,
                    padding: Edges {
                        top: 14.0,
                        right: 28.0,
                        bottom: 14.0,
                        left: 28.0,
                    },
                    width: Dimension::Content,
                    height: Dimension::Content,
                    position: Position::Absolute,
                    margin: Edges {
                        top: 110.0,
                        left: 64.0,
                        ..Default::default()
                    },
                    ..Default::default()
                },
                style: Style {
                    radius: 8.0,
                    ..fill(TEAL)
                },
                children: vec![text_node("line", "${line}", spec(34.0), Align::Start)],
                ..node("band", NodeKind::Box)
            }],
        ),
    ));
    v.push(t(
        "text-two-lines-max",
        "max_lines cutting a long binding — the ellipsis rule, whatever it is, \
         pinned",
        text_doc(
            "text-two-lines-max",
            "A long stretch of copy that would otherwise run to four or five lines in this narrow plate",
            vec![plate(vec![Node {
                content: Some(Content::Text {
                    binding: Binding {
                        expr: "${line}".into(),
                        transform: vec![],
                    },
                    font: spec(24.0),
                    align: Align::Start,
                    max_lines: Some(2),
                }),
                style: fill(WHITE),
                ..node("line", NodeKind::Text)
            }])],
        ),
    ));
    v.push(t(
        "text-coloured",
        "cream type rather than white — the fill IS the ink for a text node",
        text_doc(
            "text-coloured",
            "Cream on teal",
            vec![Node {
                style: fill(TEAL),
                ..plate(vec![Node {
                    style: fill(CREAM),
                    ..text_node("line", "${line}", spec(40.0), Align::Center)
                }])
            }],
        ),
    ));
    v.push(t(
        "text-rotated",
        "shaped text under a rotation — glyph quads through the affine",
        text_doc(
            "text-rotated",
            "Rotated type",
            vec![Node {
                transform: Transform2D {
                    rotate: -8.0,
                    ..Default::default()
                },
                ..plate(vec![text_node(
                    "line",
                    "${line}",
                    spec(40.0),
                    Align::Center,
                )])
            }],
        ),
    ));
    v.push(t(
        "text-scaled",
        "text scaled by a transform rather than by size — the atlas is NOT \
         re-rasterized, and this is what that looks like",
        text_doc(
            "text-scaled",
            "Scaled type",
            vec![Node {
                transform: Transform2D {
                    scale: [1.5, 1.5],
                    ..Default::default()
                },
                ..plate(vec![text_node(
                    "line",
                    "${line}",
                    spec(32.0),
                    Align::Center,
                )])
            }],
        ),
    ));

    v
}

/// Look one up by name.
pub fn by_name(name: &str) -> Option<Template> {
    all().into_iter().find(|t| t.name == name)
}

/// Every figure's name, in corpus order — what the browser harness iterates.
pub fn names() -> Vec<&'static str> {
    all().iter().map(|t| t.name).collect()
}

// ---------------------------------------------------------------------------
// Arming — the CPU half, shared by both targets
// ---------------------------------------------------------------------------

/// What a host needs to draw one figure. The GPU half — a device, a target, a
/// readback — stays with the host, because how you wait for a buffer differs by
/// platform and that difference is exactly what `pmg-render` keeps out.
pub struct Armed {
    pub inst: crate::SceneInstance,
    pub frame: Vec<crate::FrameNode>,
}

/// Sample data: what Design previews with, and what a corpus figure renders.
pub fn sample_data(doc: &SceneDoc) -> Data {
    doc.fields
        .iter()
        .filter_map(|f| f.sample.clone().map(|s| (f.key.clone(), s)))
        .collect()
}

struct CorpusImages {
    w: u32,
    h: u32,
    px: Arc<Vec<u8>>,
}

impl crate::scene::ImageSource for CorpusImages {
    fn image(&mut self, _key: &str) -> Option<(u32, u32, Arc<Vec<u8>>)> {
        Some((self.w, self.h, self.px.clone()))
    }
}

/// Solve, shape, and evaluate one figure at its own sample time.
///
/// `font` is the face every declared family is registered against. A figure
/// that needs one and is handed none is an error, not a blank render — see the
/// module note on skipping loudly.
pub fn arm(t: &Template, font: Option<&[u8]>) -> Result<Armed, String> {
    let mut db = pmg_text::FontDb::new();
    if let Some(bytes) = font {
        for f in &t.doc.fonts {
            db.add_face(bytes.to_vec(), f.family.clone(), f.weight)
                .map_err(|e| format!("{}: {e}", t.name))?;
        }
    } else if t.needs_font {
        return Err(format!("{} needs a font and none was given", t.name));
    }

    let data = sample_data(&t.doc);
    let measurer = pmg_text::Measurer::new(pmg_text::TextEngine::new(db));
    let solved =
        pmg_layout::solve(&t.doc, &data, &measurer).map_err(|e| format!("{}: {e}", t.name))?;
    let mut engine = measurer.into_engine();

    let (iw, ih, px) = test_image();
    let mut images = CorpusImages {
        w: iw,
        h: ih,
        px: Arc::new(px),
    };
    let inst = crate::SceneInstance::build_with(
        &t.doc,
        &solved,
        &mut crate::text_source::TextEngineGlyphs::new(&mut engine),
        &mut images,
    );

    // Timelines evaluated where the figure asks. A static figure has none, and
    // `apply_tracks` with an empty map is the identity.
    let mut values: BTreeMap<String, f32> = BTreeMap::new();
    for tl in &t.doc.timelines {
        values.extend(crate::timeline::evaluate(tl, t.t_ms));
    }
    let frame = crate::scene::apply_tracks(&inst, &values);

    Ok(Armed { inst, frame })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// M5's verification bar, as a test rather than as a promise.
    #[test]
    fn the_corpus_is_past_fifty_templates() {
        assert!(
            all().len() > 50,
            "corpus is {} templates; M5's bar is past 50",
            all().len()
        );
    }

    #[test]
    fn names_are_unique_and_file_safe() {
        let mut seen = std::collections::BTreeSet::new();
        for t in all() {
            assert!(seen.insert(t.name), "duplicate figure name {}", t.name);
            assert!(
                t.name
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
                "{} is not a file-safe name",
                t.name
            );
            assert!(
                !t.covers.is_empty(),
                "{} says nothing about what it covers",
                t.name
            );
        }
    }

    /// Every figure must be a document the compiler would accept — a corpus
    /// entry that could not be published is not testing what a node renders.
    #[test]
    fn every_figure_compiles() {
        for t in all() {
            pmg_scene::validate::compile(&t.doc)
                .unwrap_or_else(|e| panic!("{} does not compile: {e}", t.name));
        }
    }

    /// Arming is the CPU half — layout, shaping, atlas packing, timelines. It
    /// needs no GPU, so this runs everywhere and covers most of the corpus.
    #[test]
    fn every_font_free_figure_arms_and_draws_something() {
        let mut armed = 0;
        for t in all().iter().filter(|t| !t.needs_font) {
            let a = arm(t, None).unwrap_or_else(|e| panic!("{}: {e}", t.name));
            assert_eq!(
                a.frame.len(),
                a.inst.nodes.len(),
                "{}: frame and instance disagree on node count",
                t.name
            );
            assert!(
                a.inst.nodes.len() > 1,
                "{} armed to {} node(s) — it would render as nothing",
                t.name,
                a.inst.nodes.len()
            );
            armed += 1;
        }
        assert!(armed >= 45, "only {armed} font-free figures");
    }

    #[test]
    fn a_figure_that_needs_a_font_refuses_rather_than_rendering_blank() {
        let t = by_name("text-plain").expect("text-plain");
        assert!(t.needs_font);
        assert!(arm(&t, None).is_err());
    }

    /// The one figure that is supposed to be degraded, and the rest that are
    /// not. A corpus quietly full of magenta placeholders would still pass a
    /// parity diff.
    #[test]
    fn only_the_degraded_figure_is_degraded() {
        for t in all().iter().filter(|t| !t.needs_font) {
            let a = arm(t, None).unwrap();
            assert_eq!(
                a.inst.degraded,
                t.name == "degraded-unknown-kind",
                "{} degraded = {}",
                t.name,
                a.inst.degraded
            );
        }
    }

    /// visible_if removes a node from LAYOUT, so the row closes up. Asserted on
    /// the solved geometry rather than left to the eye.
    #[test]
    fn visible_if_closes_the_row_up() {
        let t = by_name("visible-if-collapsed").unwrap();
        let a = arm(&t, None).unwrap();
        let ids: Vec<&str> = a.inst.nodes.iter().map(|n| n.id.as_str()).collect();
        assert!(!ids.contains(&"dropped"), "the hidden node still laid out");
        assert!(ids.contains(&"after"), "the sibling after it vanished too");
    }

    /// The theme figure and its untokened twin must arm to identical geometry
    /// and identical colours — ADR 0021's whole claim, checked without a GPU.
    #[test]
    fn tokens_do_not_change_what_renders() {
        let a = arm(&by_name("theme-tokens").unwrap(), None).unwrap();
        let b = arm(&by_name("theme-tokens-untokened-twin").unwrap(), None).unwrap();
        assert_eq!(a.inst.nodes.len(), b.inst.nodes.len());
        for (x, y) in a.inst.nodes.iter().zip(b.inst.nodes.iter()) {
            assert_eq!(x.rect, y.rect, "geometry differs");
            // The VALUES, not the `Rgba`s: token provenance rides along into
            // the render node and is exactly the part that must not matter.
            assert_eq!(
                x.fill.as_ref().map(|c| c.rgba),
                y.fill.as_ref().map(|c| c.rgba),
                "fill differs"
            );
            assert_eq!(
                x.stroke.as_ref().map(|s| (s.color.rgba, s.width)),
                y.stroke.as_ref().map(|s| (s.color.rgba, s.width)),
                "stroke differs"
            );
        }
    }

    /// A mask node is in the instance (its transform is animated like any
    /// other node's) but draws nothing, and its siblings carry its index.
    #[test]
    fn a_mask_is_present_but_not_painted() {
        let t = by_name("mask-reveals-part-of-a-box").unwrap();
        let a = arm(&t, None).unwrap();
        let mi = a
            .inst
            .nodes
            .iter()
            .position(|n| n.id == "m")
            .expect("the mask is in the instance");
        assert!(matches!(
            a.inst.nodes[mi].content,
            crate::RenderContent::Mask
        ));
        let content = a.inst.nodes.iter().find(|n| n.id == "a").unwrap();
        assert_eq!(
            content.mask,
            Some(mi),
            "the masked sibling carries its index"
        );
        // And the marker OUTSIDE the masked group carries nothing.
        let marker = a.inst.nodes.iter().find(|n| n.id == "marker").unwrap();
        assert_eq!(marker.mask, None);
    }

    /// The mask applies to the whole subtree, not just to immediate siblings —
    /// a plate and the text inside it must be wiped together or the type
    /// survives the wipe.
    #[test]
    fn a_mask_reaches_descendants() {
        let t = by_name("mask-wipe-mid").unwrap();
        // No font here, so the text node contributes no glyphs; the plate is
        // what this asserts on.
        let d = t.doc.clone();
        let plate_masked = {
            let mut boxes_only = d.clone();
            boxes_only.fields.clear();
            strip_text(&mut boxes_only.root);
            let t2 = Template {
                doc: boxes_only,
                needs_font: false,
                ..t.clone()
            };
            let a = arm(&t2, None).unwrap();
            a.inst
                .nodes
                .iter()
                .find(|n| n.id == "plate")
                .unwrap()
                .mask
                .is_some()
        };
        assert!(plate_masked, "the plate inside the masked group is masked");
    }

    fn strip_text(n: &mut Node) {
        if matches!(n.kind, NodeKind::Text) {
            n.content = None;
            n.kind = NodeKind::Box;
        }
        for c in &mut n.children {
            strip_text(c);
        }
    }

    /// A mask that carries paint is refused, because an older reader would draw
    /// that paint straight across the graphic — the whole reason the fallback
    /// is safe.
    #[test]
    fn a_painted_mask_does_not_compile() {
        let mut t = by_name("mask-rounded").unwrap();
        fn paint(n: &mut Node) {
            if matches!(n.kind, NodeKind::Mask) {
                n.style.fill = Some(Rgba::flat(RED));
            }
            for c in &mut n.children {
                paint(c);
            }
        }
        paint(&mut t.doc.root);
        assert!(pmg_scene::validate::compile(&t.doc).is_err());
    }

    #[test]
    fn the_test_image_is_deterministic() {
        assert_eq!(test_image().2, test_image().2);
        let (w, h, px) = test_image();
        assert_eq!(px.len(), (w * h * 4) as usize);
    }
}
