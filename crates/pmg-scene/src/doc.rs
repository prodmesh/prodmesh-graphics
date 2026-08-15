//! The SceneDoc document model.
//!
//! Normative spec: `docs/specs/scene-format.md`. Where this and the spec
//! disagree, **the spec is right and this is a bug**.
//!
//! Every `Option` here is `skip_serializing_if` so that absent means absent in
//! the encoding rather than an explicit null — a document that does not use
//! shadows should not carry a `"shadow": null` into its hash.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Arbitrary CBOR/JSON, used for the forward-compatibility escape hatches.
pub type Unknown = BTreeMap<String, ciborium::Value>;

fn is_false(b: &bool) -> bool {
    !*b
}

// ---------------------------------------------------------------------------
// Document
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SceneDoc {
    pub format: String,
    pub format_version: FormatVersion,
    /// The oldest reader major version that can render this correctly.
    ///
    /// **A node refuses a template whose `min_reader` exceeds its own at
    /// assignment time, loudly** — not at take time. You find out at 10am
    /// Saturday, not 10:15 Sunday (`docs/specs/scene-format.md` §9.1).
    pub min_reader: u32,

    pub id: String,
    pub name: String,
    pub canvas: Canvas,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<Field>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fonts: Vec<FontRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assets: Vec<AssetRef>,

    pub root: Node,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub timelines: Vec<Timeline>,

    /// Forward-compat escape hatch, preserved verbatim across a read/write cycle.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub ext: Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FormatVersion {
    pub major: u32,
    pub minor: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Canvas {
    pub width: u32,
    pub height: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub safe: Option<SafeArea>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SafeArea {
    pub action: f32,
    pub title: f32,
}

// ---------------------------------------------------------------------------
// Fields and bindings
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Field {
    pub key: String,
    #[serde(rename = "type")]
    pub ty: FieldType,
    pub label: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_len: Option<usize>,
    /// What Design previews with, and what a golden image renders.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldType {
    Text,
    Number,
    Bool,
    Image,
    Video,
    Color,
    Enum,
}

/// A binding: a template string plus a chain of total transforms.
///
/// There is no arithmetic, no conditional, and no function call. This evaluates
/// on the render thread at air time, so "cannot fail" is a requirement rather
/// than a nicety (`docs/specs/scene-format.md` §5.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Binding {
    pub expr: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub transform: Vec<crate::Transform>,
}

/// The condition that collapses a lower third to one line when there is no
/// title.
///
/// A node whose `visible_if` is false is **removed from layout**, not merely
/// hidden — which is the whole point, and something interpolation cannot
/// express (`docs/specs/scene-format.md` §5.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VisibleIf {
    pub field: String,
    pub op: VisibleOp,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VisibleOp {
    NonEmpty,
    Empty,
    Eq(String),
    Ne(String),
    Gt(f64),
    Lt(f64),
    True,
    False,
}

// ---------------------------------------------------------------------------
// Assets
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssetRef {
    /// Local name used by nodes in this document.
    pub key: String,
    /// Global identity: `blake3:<hex>`.
    pub hash: String,
    pub kind: String,
}

/// **Fonts are assets.** Carrying the bytes by content hash is what makes
/// WYSIWYG survive across machines (`docs/specs/scene-format.md` §7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FontRef {
    pub family: String,
    #[serde(default = "default_weight")]
    pub weight: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asset: Option<String>,
    /// Naming a system font is allowed but Design must flag it: *this will
    /// differ between machines.*
    #[serde(default, skip_serializing_if = "is_false")]
    pub system_fallback: bool,
}

fn default_weight() -> u16 {
    400
}

// ---------------------------------------------------------------------------
// Nodes
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Node {
    /// **Stable forever.** Timeline tracks address `id/property`, so reordering
    /// the tree or renaming a layer can never break an animation. This is the
    /// most compatibility-critical rule in the format (§3.1).
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    pub kind: NodeKind,

    #[serde(default)]
    pub layout: Layout,
    #[serde(default)]
    pub transform: Transform2D,
    #[serde(default)]
    pub style: Style,

    #[serde(default, skip_serializing_if = "is_false")]
    pub clip: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visible_if: Option<VisibleIf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<Content>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<Node>,

    /// Keys this reader did not understand, kept so an older designer can open,
    /// edit, and re-save a newer document without silently deleting what it does
    /// not know about (§3.4).
    #[serde(flatten, default, skip_serializing_if = "BTreeMap::is_empty")]
    pub unknown: Unknown,
}

/// Node kinds. `Other` catches anything a newer format adds.
///
/// A reader **preserves** an unknown kind on write but **refuses to render**
/// it — magenta placeholder plus `degraded`. Rendering nothing and looking fine
/// is the dangerous behaviour (§3.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    Box,
    Text,
    Image,
    Video,
    Shape,
    Mask,
    Group,
    #[serde(untagged)]
    Other(String),
}

impl NodeKind {
    /// Can this reader draw it? See §3.4 — the answer for anything unknown is
    /// no, and loudly.
    pub fn is_renderable(&self) -> bool {
        !matches!(self, NodeKind::Other(_))
    }
}

// ---------------------------------------------------------------------------
// Layout
// ---------------------------------------------------------------------------

/// A length along one axis.
///
/// [`Dimension::Content`] is the one that matters: on a text node it calls the
/// text measurer and sizes the box to the shaped text, which — in a row, with
/// gap and padding — *is* the shrink-and-grow lower third (§4.1).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub enum Dimension {
    Px(f32),
    Percent(f32),
    /// Flexbox's own default, and therefore this enum's.
    #[default]
    Auto,
    Content,
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Edges {
    #[serde(default)]
    pub top: f32,
    #[serde(default)]
    pub right: f32,
    #[serde(default)]
    pub bottom: f32,
    #[serde(default)]
    pub left: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    #[default]
    Row,
    Column,
    RowReverse,
    ColumnReverse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Justify {
    #[default]
    Start,
    Center,
    End,
    SpaceBetween,
    SpaceAround,
    SpaceEvenly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Align {
    Start,
    Center,
    End,
    #[default]
    Stretch,
    Baseline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Position {
    #[default]
    Relative,
    Absolute,
}

/// **`layout.*` forces a relayout; `transform.*` does not.**
///
/// Animate transforms. A responsive lower third sizes itself once, when data
/// binds, and reveals with a transform and a clip. The editor warns when a
/// timeline targets a `layout.*` property (§3.2).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Layout {
    #[serde(default)]
    pub direction: Direction,
    #[serde(default)]
    pub justify: Justify,
    #[serde(default)]
    pub align: Align,
    #[serde(default)]
    pub grow: f32,
    #[serde(default)]
    pub shrink: f32,
    #[serde(default)]
    pub width: Dimension,
    #[serde(default)]
    pub height: Dimension,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_width: Option<Dimension>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_width: Option<Dimension>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_height: Option<Dimension>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_height: Option<Dimension>,
    #[serde(default)]
    pub margin: Edges,
    #[serde(default)]
    pub padding: Edges,
    #[serde(default)]
    pub gap: f32,
    #[serde(default)]
    pub position: Position,
}

// ---------------------------------------------------------------------------
// Transform and style
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Transform2D {
    #[serde(default)]
    pub translate: [f32; 2],
    #[serde(default)]
    pub rotate: f32,
    #[serde(default = "unit_scale")]
    pub scale: [f32; 2],
    #[serde(default = "center_anchor")]
    pub anchor: [f32; 2],
    #[serde(default)]
    pub skew: [f32; 2],
}

fn unit_scale() -> [f32; 2] {
    [1.0, 1.0]
}
fn center_anchor() -> [f32; 2] {
    [0.5, 0.5]
}

impl Default for Transform2D {
    fn default() -> Self {
        Self {
            translate: [0.0, 0.0],
            rotate: 0.0,
            scale: unit_scale(),
            anchor: center_anchor(),
            skew: [0.0, 0.0],
        }
    }
}

/// Linear-light **straight** RGBA as authored. Premultiplication happens in the
/// renderer's working space, not in the document (ADR 0012).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rgba {
    pub rgba: [f32; 4],
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Style {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill: Option<Rgba>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stroke: Option<Rgba>,
    #[serde(default)]
    pub radius: f32,
    #[serde(default = "one")]
    pub opacity: f32,
}

/// Hand-written, NOT derived.
///
/// `#[serde(default = "one")]` and `#[derive(Default)]` are different
/// mechanisms: the serde attribute fills a *missing field during
/// deserialization*, while `Default::default()` uses `f32::default()` — which
/// is **0.0**.
///
/// Deriving here made every `Style { fill: .., ..Default::default() }` fully
/// transparent, and the renderer dutifully drew a whole frame of nothing. Any
/// field whose serde default is non-zero needs a hand-written `Default` beside
/// it.
impl Default for Style {
    fn default() -> Self {
        Self {
            fill: None,
            stroke: None,
            radius: 0.0,
            opacity: 1.0,
        }
    }
}

fn one() -> f32 {
    1.0
}

// ---------------------------------------------------------------------------
// Content
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Content {
    Text {
        binding: Binding,
        font: FontSpec,
        #[serde(default)]
        align: Align,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_lines: Option<u32>,
    },
    Image {
        asset_key: String,
    },
    Shape {
        shape: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FontSpec {
    pub family: String,
    #[serde(default = "default_weight")]
    pub weight: u16,
    pub size: f32,
    /// OpenType features. **`tnum` matters** — clocks and scores jitter without
    /// tabular figures (§3.3).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub features: Vec<String>,
    #[serde(default)]
    pub tracking: f32,
    #[serde(default = "default_line_height")]
    pub line_height: f32,
}

fn default_line_height() -> f32 {
    1.2
}

// ---------------------------------------------------------------------------
// Timelines
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Timeline {
    /// `In` and `Out` are reserved and have engine meaning. Everything else is
    /// user-named and played by an explicit cue (§6.1).
    pub name: String,
    pub duration_ms: f32,
    #[serde(default)]
    pub on_complete: OnComplete,
    #[serde(default, rename = "loop", skip_serializing_if = "is_false")]
    pub looping: bool,
    pub tracks: Vec<Track>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnComplete {
    #[default]
    Hold,
    Evict,
    Goto(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Track {
    /// `<node_id>/<property path>`, e.g. `title/transform.translate.x`.
    ///
    /// Addressing by stable id is what makes an animation survive a designer
    /// refactoring the tree (§6.2).
    pub target: String,
    #[serde(default)]
    pub blend: Blend,
    pub keys: Vec<pmg_anim::Key>,
}

/// Only `Replace` is implemented in v1. The field exists so `Add` is additive
/// later rather than a format break (§6.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Blend {
    #[default]
    Replace,
    Add,
}

impl Track {
    /// Does this track animate a `layout.*` property?
    ///
    /// Not an error — the compiler records it as a warning in the manifest, and
    /// the editor surfaces it. Animating layout relayouts every frame; animating
    /// transform is free (§3.2).
    pub fn animates_layout(&self) -> bool {
        self.target
            .split_once('/')
            .map(|(_, prop)| prop.starts_with("layout."))
            .unwrap_or(false)
    }

    /// The node id this track addresses.
    pub fn node_id(&self) -> &str {
        self.target.split_once('/').map(|(id, _)| id).unwrap_or("")
    }
}

// ---------------------------------------------------------------------------
// Tree walking
// ---------------------------------------------------------------------------

impl Node {
    /// Depth-first iteration over this node and its descendants.
    pub fn walk<'a>(&'a self, f: &mut impl FnMut(&'a Node)) {
        f(self);
        for c in &self.children {
            c.walk(f);
        }
    }
}

impl SceneDoc {
    /// Every node id in the document, in tree order.
    pub fn node_ids(&self) -> Vec<&str> {
        let mut out = Vec::new();
        self.root.walk(&mut |n| out.push(n.id.as_str()));
        out
    }

    /// Find a node by its stable id.
    pub fn node(&self, id: &str) -> Option<&Node> {
        let mut found = None;
        self.root.walk(&mut |n| {
            if found.is_none() && n.id == id {
                found = Some(n);
            }
        });
        found
    }
}
