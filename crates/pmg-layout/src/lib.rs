//! Flexbox layout over a [`TextMeasure`] trait.
//!
//! Spec: `docs/specs/scene-format.md` §4. Decisions: ADR 0005 (the model),
//! ADR 0016 (taffy as the solver).
//!
//! In the **pure set**: no I/O, no platform, no ambient state. Given a document,
//! item data, and a text measurer, [`solve`] returns resolved rectangles — the
//! same numbers on every platform and both build targets. This is the half of
//! ADR 0001's promise that *is* guaranteed, and it is compared as numbers by
//! spike S1 before anything is rendered.
//!
//! # The rule this crate exists to make cheap
//!
//! `layout.*` affects siblings and forces a relayout. `transform.*` does not.
//! A responsive lower third sizes itself **once**, when data binds, and reveals
//! with a transform and a clip. Nothing here runs per frame.
//!
//! # Taffy does not appear in the public API
//!
//! Callers see our node ids and our rectangles. The SceneDoc → taffy
//! translation lives in one function ([`build`]) so the mapping is readable in
//! one place and the engine stays replaceable (ADR 0016 §Seam).

#![forbid(unsafe_code)]

use pmg_scene::bind::{self, Data};
use pmg_scene::doc::{Content, Dimension as SceneDim, Direction, Edges, FontSpec, Node, SceneDoc};
use std::collections::BTreeMap;
use taffy::prelude as tf;

/// A resolved rectangle in design space, absolute from the canvas origin.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn right(&self) -> f32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
}

/// Text measurement, supplied by `pmg-text`.
///
/// A trait rather than a dependency so this crate stays testable with a
/// deterministic stub — and so the text stack can be replaced (spike S2
/// contemplates exactly that) without the solver noticing.
pub trait TextMeasure {
    /// Measure a run, optionally constrained to `max_width`. Returns
    /// `(width, height)` in design-space pixels.
    fn measure(&self, text: &str, font: &FontSpec, max_width: Option<f32>) -> (f32, f32);
}

/// The result of laying out a document against one Item's data.
#[derive(Debug, Clone, Default)]
pub struct Solved {
    /// Absolute rectangle per node id, for every node that took part.
    pub rects: BTreeMap<String, Rect>,
    /// Nodes removed by `visible_if`.
    ///
    /// **Removed, not hidden.** They occupy no space and contribute nothing to
    /// a content-sized parent, which is what collapses a lower third to one
    /// line when there is no title (ADR 0005 §5.3).
    pub omitted: Vec<String>,
    /// The text each text node resolved to, after binding and transforms.
    /// Carried through so the renderer does not re-evaluate bindings.
    pub text: BTreeMap<String, String>,
}

impl Solved {
    pub fn rect(&self, id: &str) -> Option<Rect> {
        self.rects.get(id).copied()
    }
    pub fn was_omitted(&self, id: &str) -> bool {
        self.omitted.iter().any(|s| s == id)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LayoutError {
    #[error("layout failed: {0}")]
    Taffy(String),
}

/// Lay out `doc` against `data`.
pub fn solve(
    doc: &SceneDoc,
    data: &Data,
    measure: &dyn TextMeasure,
) -> Result<Solved, LayoutError> {
    let mut tree: tf::TaffyTree<Ctx> = tf::TaffyTree::new();
    let mut out = Solved::default();

    let root = build(&mut tree, &doc.root, data, &mut out)?;

    tree.compute_layout_with_measure(
        root,
        tf::Size {
            width: tf::AvailableSpace::Definite(doc.canvas.width as f32),
            height: tf::AvailableSpace::Definite(doc.canvas.height as f32),
        },
        |known, available, _node, ctx, _style| measure_leaf(known, available, ctx, measure),
    )
    .map_err(|e| LayoutError::Taffy(e.to_string()))?;

    collect(&tree, root, 0.0, 0.0, &mut out);
    Ok(out)
}

/// What a taffy leaf needs to measure itself.
struct Ctx {
    id: String,
    text: Option<(String, FontSpec)>,
}

/// Measure a text leaf.
///
/// Honours a known width so wrapping is resolved against the box the solver has
/// already decided on, rather than against the unconstrained string.
fn measure_leaf(
    known: tf::Size<Option<f32>>,
    available: tf::Size<tf::AvailableSpace>,
    ctx: Option<&mut Ctx>,
    measure: &dyn TextMeasure,
) -> tf::Size<f32> {
    let Some(ctx) = ctx else {
        return tf::Size::ZERO;
    };
    let Some((text, font)) = &ctx.text else {
        return tf::Size::ZERO;
    };
    if let (Some(w), Some(h)) = (known.width, known.height) {
        return tf::Size {
            width: w,
            height: h,
        };
    }
    let max_width = known.width.or(match available.width {
        tf::AvailableSpace::Definite(w) => Some(w),
        // MinContent/MaxContent: measure unconstrained and let flexbox decide.
        _ => None,
    });
    let (w, h) = measure.measure(text, font, max_width);
    tf::Size {
        width: known.width.unwrap_or(w),
        height: known.height.unwrap_or(h),
    }
}

/// SceneDoc → taffy, in one place.
///
/// A node whose `visible_if` is false is simply never inserted — the removal
/// happens here, before the tree exists, which is why a hidden node cannot
/// influence a content-sized parent.
///
/// Note there is no measurer parameter: measurement happens later, in the
/// callback `compute_layout_with_measure` invokes, because taffy decides *when*
/// a node needs measuring. This function only records what a leaf would measure.
fn build(
    tree: &mut tf::TaffyTree<Ctx>,
    node: &Node,
    data: &Data,
    out: &mut Solved,
) -> Result<tf::NodeId, LayoutError> {
    let style = to_taffy_style(node);

    // Resolve text once, here, and carry it in Solved so the renderer never
    // re-evaluates a binding.
    let text = match &node.content {
        Some(Content::Text { binding, font, .. }) => {
            let resolved = bind::eval(&binding.expr, &binding.transform, data);
            out.text.insert(node.id.clone(), resolved.clone());
            Some((resolved, font.clone()))
        }
        _ => None,
    };

    let mut children = Vec::new();
    for child in &node.children {
        // visible_if is applied BEFORE insertion. A node that fails it is never
        // in the tree, so it occupies no space and does not widen its parent.
        if let Some(cond) = &child.visible_if {
            if !bind::visible(cond, data) {
                child.walk(&mut |n| out.omitted.push(n.id.clone()));
                continue;
            }
        }
        children.push(build(tree, child, data, out)?);
    }

    let ctx = Ctx {
        id: node.id.clone(),
        text,
    };
    let id = if children.is_empty() {
        tree.new_leaf_with_context(style, ctx)
    } else {
        let n = tree
            .new_with_children(style, &children)
            .map_err(|e| LayoutError::Taffy(e.to_string()))?;
        tree.set_node_context(n, Some(ctx))
            .map_err(|e| LayoutError::Taffy(e.to_string()))?;
        Ok(n)
    }
    .map_err(|e| LayoutError::Taffy(e.to_string()))?;

    Ok(id)
}

fn to_dim(d: SceneDim) -> tf::Dimension {
    match d {
        SceneDim::Px(v) => tf::Dimension::length(v),
        SceneDim::Percent(p) => tf::Dimension::percent(p / 100.0),
        // `Content` maps to auto: flexbox's own content sizing. On a text leaf
        // the measure function supplies the size; on a box it sizes to children.
        // That is the whole shrink-and-grow mechanism, and it is flexbox's
        // behaviour rather than something we add on top.
        SceneDim::Auto | SceneDim::Content => tf::Dimension::auto(),
    }
}

fn to_opt_dim(d: Option<SceneDim>) -> tf::Dimension {
    d.map(to_dim).unwrap_or_else(tf::Dimension::auto)
}

fn to_rect(e: Edges) -> tf::Rect<tf::LengthPercentage> {
    tf::Rect {
        left: tf::LengthPercentage::length(e.left),
        right: tf::LengthPercentage::length(e.right),
        top: tf::LengthPercentage::length(e.top),
        bottom: tf::LengthPercentage::length(e.bottom),
    }
}

fn to_taffy_style(node: &Node) -> tf::Style {
    use pmg_scene::doc::{Align, Justify, Position};
    let l = &node.layout;
    tf::Style {
        display: tf::Display::Flex,
        position: match l.position {
            Position::Relative => tf::Position::Relative,
            Position::Absolute => tf::Position::Absolute,
        },
        flex_direction: match l.direction {
            Direction::Row => tf::FlexDirection::Row,
            Direction::Column => tf::FlexDirection::Column,
            Direction::RowReverse => tf::FlexDirection::RowReverse,
            Direction::ColumnReverse => tf::FlexDirection::ColumnReverse,
        },
        // Taffy 0.13 models alignment as structs with associated constants
        // rather than plain enums; FLEX_START and friends are the flexbox
        // (rather than the grid) spellings, which is what this model wants.
        justify_content: Some(match l.justify {
            Justify::Start => tf::JustifyContent::FLEX_START,
            Justify::Center => tf::JustifyContent::CENTER,
            Justify::End => tf::JustifyContent::FLEX_END,
            Justify::SpaceBetween => tf::JustifyContent::SPACE_BETWEEN,
            Justify::SpaceAround => tf::JustifyContent::SPACE_AROUND,
            Justify::SpaceEvenly => tf::JustifyContent::SPACE_EVENLY,
        }),
        align_items: Some(match l.align {
            Align::Start => tf::AlignItems::FLEX_START,
            Align::Center => tf::AlignItems::CENTER,
            Align::End => tf::AlignItems::FLEX_END,
            Align::Stretch => tf::AlignItems::STRETCH,
            Align::Baseline => tf::AlignItems::BASELINE,
        }),
        flex_grow: l.grow,
        flex_shrink: l.shrink,
        size: tf::Size {
            width: to_dim(l.width),
            height: to_dim(l.height),
        },
        min_size: tf::Size {
            width: to_opt_dim(l.min_width),
            height: to_opt_dim(l.min_height),
        },
        max_size: tf::Size {
            width: to_opt_dim(l.max_width),
            height: to_opt_dim(l.max_height),
        },
        margin: tf::Rect {
            left: tf::LengthPercentageAuto::length(l.margin.left),
            right: tf::LengthPercentageAuto::length(l.margin.right),
            top: tf::LengthPercentageAuto::length(l.margin.top),
            bottom: tf::LengthPercentageAuto::length(l.margin.bottom),
        },
        padding: to_rect(l.padding),
        gap: tf::Size {
            width: tf::LengthPercentage::length(l.gap),
            height: tf::LengthPercentage::length(l.gap),
        },
        ..Default::default()
    }
}

/// Walk the solved tree, accumulating absolute positions.
///
/// Taffy reports each node's location relative to its parent; the renderer wants
/// absolute, so the offset is carried down here rather than recomputed later.
fn collect(tree: &tf::TaffyTree<Ctx>, node: tf::NodeId, ox: f32, oy: f32, out: &mut Solved) {
    let Ok(l) = tree.layout(node) else { return };
    let (x, y) = (ox + l.location.x, oy + l.location.y);

    if let Some(ctx) = tree.get_node_context(node) {
        out.rects.insert(
            ctx.id.clone(),
            Rect {
                x,
                y,
                w: l.size.width,
                h: l.size.height,
            },
        );
    }
    if let Ok(children) = tree.children(node) {
        for c in children {
            collect(tree, c, x, y, out);
        }
    }
}

/// A deterministic stub measurer, for tests and for `--selftest`.
///
/// **Not an approximation of real text.** It is a fixed function of character
/// count so layout can be asserted as exact numbers before `pmg-text` exists —
/// and so a layout regression is never confused with a shaping change.
#[derive(Debug, Clone, Copy)]
pub struct StubMeasure {
    /// Advance per character, as a fraction of font size.
    pub advance_ratio: f32,
}

impl Default for StubMeasure {
    fn default() -> Self {
        Self { advance_ratio: 0.5 }
    }
}

impl TextMeasure for StubMeasure {
    fn measure(&self, text: &str, font: &FontSpec, max_width: Option<f32>) -> (f32, f32) {
        let advance = font.size * self.advance_ratio;
        let natural = text.chars().count() as f32 * advance;
        let line_h = font.size * font.line_height;
        match max_width {
            Some(mw) if mw > 0.0 && natural > mw => {
                let per_line = (mw / advance).floor().max(1.0);
                let lines = (text.chars().count() as f32 / per_line).ceil();
                (mw, lines * line_h)
            }
            _ => (natural, line_h),
        }
    }
}

pub fn selftest() -> Result<(), String> {
    use pmg_scene::fixture;

    let doc = fixture::lower_third();
    let m = StubMeasure::default();

    let with_title: Data = [
        ("name".to_string(), "Pastor Dan Whitfield".to_string()),
        ("title".to_string(), "Lead Pastor".to_string()),
    ]
    .into_iter()
    .collect();

    let solved = solve(&doc, &with_title, &m).map_err(|e| e.to_string())?;

    let band = solved.rect("band").ok_or("band did not lay out")?;
    let name = solved.rect("name").ok_or("name did not lay out")?;
    let title = solved.rect("title").ok_or("title did not lay out")?;

    if band.w <= 0.0 || band.h <= 0.0 {
        return Err(format!("band collapsed: {band:?}"));
    }
    // The band must contain its children — a content-sized box that does not
    // wrap its content is the bug this whole mechanism exists to avoid.
    if name.right() > band.right() + 0.01 || title.bottom() > band.bottom() + 0.01 {
        return Err(format!(
            "band {band:?} does not contain {name:?} / {title:?}"
        ));
    }

    // --- The shrink-and-grow property ------------------------------------
    let short: Data = [
        ("name".to_string(), "Dan".to_string()),
        ("title".to_string(), "Lead Pastor".to_string()),
    ]
    .into_iter()
    .collect();
    let narrow = solve(&doc, &short, &m).map_err(|e| e.to_string())?;
    let narrow_band = narrow.rect("band").ok_or("band missing")?;
    if narrow_band.w >= band.w {
        return Err(format!(
            "band did not shrink with a shorter name: {} vs {}",
            narrow_band.w, band.w
        ));
    }

    // --- visible_if removes rather than hides -----------------------------
    let no_title: Data = [
        ("name".to_string(), "Pastor Dan Whitfield".to_string()),
        ("title".to_string(), "  ".to_string()),
    ]
    .into_iter()
    .collect();
    let collapsed = solve(&doc, &no_title, &m).map_err(|e| e.to_string())?;
    if collapsed.rect("title").is_some() {
        return Err("an omitted node still produced a rectangle".into());
    }
    if !collapsed.was_omitted("title") {
        return Err("title was not recorded as omitted".into());
    }
    let collapsed_band = collapsed.rect("band").ok_or("band missing")?;
    if collapsed_band.h >= band.h {
        return Err(format!(
            "band did not collapse without a title: {} vs {}",
            collapsed_band.h, band.h
        ));
    }

    // --- Determinism -------------------------------------------------------
    let again = solve(&doc, &with_title, &m).map_err(|e| e.to_string())?;
    if again.rect("band") != Some(band) || again.rect("name") != Some(name) {
        return Err("layout is not deterministic".into());
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
}
