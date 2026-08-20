//! Interaction geometry — the third row of ADR 0019's table.
//!
//! Hit-testing, handle positions, and drag/snap math over a [`SceneInstance`],
//! answering in canvas coordinates. **Pure geometry over a document**: nothing
//! here knows about React, the server, or a session — which is what lets the
//! same functions serve a native editor later without being a port.
//!
//! # Why this lives beside the renderer
//!
//! A hit test in JavaScript would be a second implementation of the thing that
//! must never disagree (ADR 0019, and the same argument that keeps the server
//! from parsing SceneDocs). A box's rectangle after flexbox, a rotated node's
//! true bounds, whether a click landed inside a rounded corner: every one of
//! those answers already exists here, in the affine and SDF math the renderer
//! draws with — [`world_transforms`] is the SAME composition `draw` uploads,
//! and [`hit_test`] evaluates the SAME rounded-box distance the fragment
//! shader shades. Exactness is the requirement: a near-enough hit test is a
//! designer clicking a box and selecting its neighbour, and it would be blamed
//! on the UI rather than on the maths.
//!
//! # What edits mean, in a flexbox document
//!
//! The scene format has no free x/y — position comes from the solver, which is
//! the whole responsive premise (ADR 0005). So the designer's gestures map to
//! the two things the format can honestly express:
//!
//! - **Move** edits the node's **margins**. For a `position: absolute` node
//!   this is free placement (it is out of flow; margins offset it from its
//!   static position and nothing else moves). For a relative node it adjusts
//!   spacing within the flex flow — siblings shift, which is flexbox telling
//!   the truth rather than the editor hiding it.
//! - **Resize** sets `width`/`height` to fixed pixels. Dragging a handle on a
//!   `Content`-sized node converts it to fixed size — a real authoring
//!   decision the designer just made by dragging, not a silent loss.

use crate::scene::{FrameNode, RenderContent, SceneInstance};
use pmg_scene::doc::{Dimension, Node, Position, SceneDoc};
use serde::Serialize;

/// A 2×3 affine: column-major 2×2 plus a translation, matching `mat2x2` in
/// WGSL and the uniform layout `draw` uploads.
pub type Affine = ([f32; 4], [f32; 2]);

/// Build the 2×3 affine for a node: scale and rotate about the anchor, then
/// translate.
///
/// The anchor is a fraction of the node's own rect, so a lower third anchored
/// at `[0, 0.5]` rotates about its left edge rather than its centre — which is
/// what a designer means by "hinge it on the left".
pub fn affine(t: &pmg_scene::doc::Transform2D, rect: &pmg_layout::Rect) -> Affine {
    let (sin, cos) = t.rotate.to_radians().sin_cos();
    let (sx, sy) = (t.scale[0], t.scale[1]);

    // Rotation composed with scale.
    let m = [cos * sx, sin * sx, -sin * sy, cos * sy];

    // Pivot in absolute design space.
    let px = rect.x + rect.w * t.anchor[0];
    let py = rect.y + rect.h * t.anchor[1];

    // Translate so the pivot maps to itself, then apply the authored offset.
    let tx = px - (m[0] * px + m[2] * py) + t.translate[0];
    let ty = py - (m[1] * px + m[3] * py) + t.translate[1];
    (m, [tx, ty])
}

/// Compose two affines: `outer` applied after `inner`.
pub fn compose(outer: Affine, inner: Affine) -> Affine {
    let (a, at) = outer;
    let (b, bt) = inner;
    // Column-major: m*p = (m[0]*x + m[2]*y, m[1]*x + m[3]*y).
    let m = [
        a[0] * b[0] + a[2] * b[1],
        a[1] * b[0] + a[3] * b[1],
        a[0] * b[2] + a[2] * b[3],
        a[1] * b[2] + a[3] * b[3],
    ];
    let t = [
        a[0] * bt[0] + a[2] * bt[1] + at[0],
        a[1] * bt[0] + a[3] * bt[1] + at[1],
    ];
    (m, t)
}

/// Every node's world transform: its own affine composed with its ancestors'.
///
/// This is the SAME composition the renderer uploads per node — `draw` calls
/// this function, so a hit test and a drawn pixel cannot disagree about where
/// a node is. Parents always precede children in `nodes` (flatten is
/// depth-first, pushing before recursing), so one forward pass is enough.
pub fn world_transforms(inst: &SceneInstance, frame: &[FrameNode]) -> Vec<Affine> {
    let mut world: Vec<Affine> = Vec::with_capacity(inst.nodes.len());
    for (i, n) in inst.nodes.iter().enumerate() {
        let f = frame.get(i).copied().unwrap_or(FrameNode {
            transform: n.base_transform,
            opacity: n.opacity,
            rect: None,
        });
        // The anchor is a fraction of the node's rect, so a morphing node has
        // to rotate and scale about its CURRENT rect, not the one layout
        // produced.
        let local = affine(&f.transform, &f.rect.unwrap_or(n.rect));
        world.push(match n.parent.and_then(|p| world.get(p).copied()) {
            Some(parent) => compose(parent, local),
            None => local,
        });
    }
    world
}

fn apply(a: &Affine, p: [f32; 2]) -> [f32; 2] {
    let (m, t) = a;
    [
        m[0] * p[0] + m[2] * p[1] + t[0],
        m[1] * p[0] + m[3] * p[1] + t[1],
    ]
}

fn invert(a: &Affine) -> Option<Affine> {
    let (m, t) = a;
    let det = m[0] * m[3] - m[2] * m[1];
    if det.abs() < 1e-6 {
        return None; // scaled to nothing; there is no inside to hit
    }
    let inv = [m[3] / det, -m[1] / det, -m[2] / det, m[0] / det];
    let it = [
        -(inv[0] * t[0] + inv[2] * t[1]),
        -(inv[1] * t[0] + inv[3] * t[1]),
    ];
    Some((inv, it))
}

/// The rounded-box signed distance — the same function the fragment shader
/// evaluates (`sd_rounded_box` in scene.wgsl), so the clickable shape IS the
/// drawn shape, corner notches included.
fn sd_rounded_box(p: [f32; 2], half: [f32; 2], r: f32) -> f32 {
    let qx = p[0].abs() - half[0] + r;
    let qy = p[1].abs() - half[1] + r;
    let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
    outside + qx.max(qy).min(0.0) - r
}

/// How far outside the drawn edge a click still lands, in design pixels.
///
/// The visible boundary is half-coverage (sd = 0); the extra half pixel is the
/// antialiased fringe. A designer aiming at a 1 px rule should not need pixel
/// aim, and half a pixel of slop is still geometry, not guesswork.
const CLICK_SLOP: f32 = 0.5;

/// The node under a point, topmost first — the reverse of paint order,
/// because what a designer sees in front is what they mean to click.
///
/// Structural nodes — an unfilled box with no content — are transparent to
/// clicks: they draw nothing, so hitting them would select something
/// invisible. (Selecting structure is what a layer tree is for.) Clipping
/// ancestors are honoured the same axis-aligned way the renderer's scissors
/// are, so a click on a pixel the clip removed does not select the node whose
/// pixels are not there.
pub fn hit_test(inst: &SceneInstance, frame: &[FrameNode], x: f32, y: f32) -> Option<usize> {
    let world = world_transforms(inst, frame);
    let clips = clip_rects(inst, frame, &world);

    for i in (0..inst.nodes.len()).rev() {
        let n = &inst.nodes[i];
        // A stroked box draws, so it is clickable; a shadow is not a click
        // target — it is behind the node, not of it.
        if n.fill.is_none() && n.stroke.is_none() && matches!(n.content, RenderContent::Box) {
            continue;
        }
        if let Some(c) = clips[i] {
            if x < c[0] || y < c[1] || x > c[2] || y > c[3] {
                continue;
            }
        }
        let Some(inv) = invert(&world[i]) else {
            continue;
        };
        let local = apply(&inv, [x, y]);
        let rect = frame
            .get(i)
            .and_then(|f| f.rect)
            .unwrap_or(inst.nodes[i].rect);
        let half = [rect.w * 0.5, rect.h * 0.5];
        let centered = [local[0] - (rect.x + half[0]), local[1] - (rect.y + half[1])];
        let r = n.radius.min(half[0].min(half[1]));
        if sd_rounded_box(centered, half, r) <= CLICK_SLOP {
            return Some(i);
        }
    }
    None
}

/// Scissor per node: the intersection of every clipping ancestor, as an
/// axis-aligned world rect — exact for translation and scale, an over-estimate
/// under rotation. The same rule `draw` uses for its scissors, shared so the
/// hit test cannot disagree with what was drawn.
pub fn clip_rects(
    inst: &SceneInstance,
    frame: &[FrameNode],
    world: &[Affine],
) -> Vec<Option<[f32; 4]>> {
    let mut clips: Vec<Option<[f32; 4]>> = Vec::with_capacity(inst.nodes.len());
    for (i, n) in inst.nodes.iter().enumerate() {
        let inherited = n.parent.and_then(|p| clips.get(p).copied().flatten());
        let mine = if n.clip {
            let r = frame.get(i).and_then(|f| f.rect).unwrap_or(n.rect);
            let a = &world[i];
            let pts = [
                (r.x, r.y),
                (r.x + r.w, r.y),
                (r.x, r.y + r.h),
                (r.x + r.w, r.y + r.h),
            ]
            .map(|(x, y)| apply(a, [x, y]));
            let xs = pts.map(|p| p[0]);
            let ys = pts.map(|p| p[1]);
            Some([
                xs.iter().copied().fold(f32::MAX, f32::min),
                ys.iter().copied().fold(f32::MAX, f32::min),
                xs.iter().copied().fold(f32::MIN, f32::max),
                ys.iter().copied().fold(f32::MIN, f32::max),
            ])
        } else {
            None
        };
        clips.push(match (inherited, mine) {
            (Some(a), Some(b)) => Some([
                a[0].max(b[0]),
                a[1].max(b[1]),
                a[2].min(b[2]),
                a[3].min(b[3]),
            ]),
            (Some(a), None) => Some(a),
            (None, b) => b,
        });
    }
    clips
}

/// Where the selection chrome goes, in canvas coordinates.
///
/// `corners` are the node's rect corners through its world transform, in
/// order: top-left, top-right, bottom-right, bottom-left. React draws the
/// outline through them — a polygon, so a rotated node gets a rotated outline
/// rather than a lying axis-aligned one. `bbox` is the axis-aligned bounds,
/// which is what snapping and the inspector's x/y/w/h readout speak.
#[derive(Debug, Clone, Serialize)]
pub struct Handles {
    pub id: String,
    pub corners: [[f32; 2]; 4],
    pub bbox: [f32; 4], // x, y, w, h
    pub rotated: bool,
}

pub fn handles_for(inst: &SceneInstance, frame: &[FrameNode], idx: usize) -> Handles {
    let world = world_transforms(inst, frame);
    let a = &world[idx];
    let rect = frame
        .get(idx)
        .and_then(|f| f.rect)
        .unwrap_or(inst.nodes[idx].rect);
    let corners = [
        apply(a, [rect.x, rect.y]),
        apply(a, [rect.x + rect.w, rect.y]),
        apply(a, [rect.x + rect.w, rect.y + rect.h]),
        apply(a, [rect.x, rect.y + rect.h]),
    ];
    let xs = corners.map(|p| p[0]);
    let ys = corners.map(|p| p[1]);
    let (x0, y0) = (
        xs.iter().copied().fold(f32::MAX, f32::min),
        ys.iter().copied().fold(f32::MAX, f32::min),
    );
    let (x1, y1) = (
        xs.iter().copied().fold(f32::MIN, f32::max),
        ys.iter().copied().fold(f32::MIN, f32::max),
    );
    // Axis-aligned when the world matrix has no off-diagonal terms.
    let (m, _) = a;
    let rotated = m[1].abs() > 1e-4 || m[2].abs() > 1e-4;
    Handles {
        id: inst.nodes[idx].id.clone(),
        corners,
        bbox: [x0, y0, x1 - x0, y1 - y0],
        rotated,
    }
}

// ---------------------------------------------------------------------------
// Drag
// ---------------------------------------------------------------------------

/// Which part of the selection the pointer grabbed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handle {
    Move,
    N,
    S,
    E,
    W,
    Ne,
    Nw,
    Se,
    Sw,
}

impl Handle {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "move" => Self::Move,
            "n" => Self::N,
            "s" => Self::S,
            "e" => Self::E,
            "w" => Self::W,
            "ne" => Self::Ne,
            "nw" => Self::Nw,
            "se" => Self::Se,
            "sw" => Self::Sw,
            _ => return None,
        })
    }
    fn moves_left(self) -> bool {
        matches!(self, Self::W | Self::Nw | Self::Sw)
    }
    fn moves_right(self) -> bool {
        matches!(self, Self::E | Self::Ne | Self::Se)
    }
    fn moves_top(self) -> bool {
        matches!(self, Self::N | Self::Ne | Self::Nw)
    }
    fn moves_bottom(self) -> bool {
        matches!(self, Self::S | Self::Se | Self::Sw)
    }
}

/// One drag in progress. The gesture lives in React; THIS — where the
/// geometry says the node now is — lives here, and the split is deliberate
/// (ADR 0019: "drag state has two homes").
#[derive(Debug, Clone)]
pub struct Drag {
    pub node: usize,
    pub handle: Handle,
    start: [f32; 2],
    orig_bbox: [f32; 4],
    /// The node's own solved size, pre-transform — what a resize edits.
    orig_size: [f32; 2],
    /// World scale along each axis, so a bbox delta maps back to node units.
    scale: [f32; 2],
}

/// What a drag currently proposes. `rect` is the ghost React draws; `dx`/`dy`
/// or `w`/`h` are what an [`apply_move`] / [`apply_resize`] would write; the
/// guides are the snap lines that earned their keep, for drawing.
#[derive(Debug, Clone, Serialize)]
pub struct DragResult {
    pub rect: [f32; 4],
    pub dx: f32,
    pub dy: f32,
    pub w: f32,
    pub h: f32,
    pub guides: Vec<Guide>,
}

/// One snap line: `axis` 0 is a vertical line at x = `at`, 1 horizontal.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct Guide {
    pub axis: u8,
    pub at: f32,
}

/// How close an edge must come to a candidate before it snaps, design px.
const SNAP_THRESHOLD: f32 = 5.0;

pub fn begin_drag(
    inst: &SceneInstance,
    frame: &[FrameNode],
    idx: usize,
    handle: Handle,
    x: f32,
    y: f32,
) -> Drag {
    let h = handles_for(inst, frame, idx);
    let rect = frame
        .get(idx)
        .and_then(|f| f.rect)
        .unwrap_or(inst.nodes[idx].rect);
    // How many canvas px one node px moves: bbox extent over rect extent.
    // Identity for the unrotated, unscaled common case.
    let scale = [
        if rect.w > 0.0 {
            h.bbox[2] / rect.w
        } else {
            1.0
        },
        if rect.h > 0.0 {
            h.bbox[3] / rect.h
        } else {
            1.0
        },
    ];
    Drag {
        node: idx,
        handle,
        start: [x, y],
        orig_bbox: h.bbox,
        orig_size: [rect.w, rect.h],
        scale,
    }
}

/// Every x and y a dragged edge might want to line up with: the canvas edges
/// and centre, and the bbox edges and centres of every node that is not the
/// dragged node or carried along inside it.
fn snap_candidates(
    inst: &SceneInstance,
    frame: &[FrameNode],
    dragged: usize,
) -> (Vec<f32>, Vec<f32>) {
    let world = world_transforms(inst, frame);

    // The dragged node and its descendants move together; snapping a parent
    // to its own child is snapping it to itself.
    let mut moving = vec![false; inst.nodes.len()];
    moving[dragged] = true;
    for (i, n) in inst.nodes.iter().enumerate() {
        if let Some(p) = n.parent {
            if moving[p] {
                moving[i] = true;
            }
        }
    }

    let (w, h) = (inst.canvas_w as f32, inst.canvas_h as f32);
    let mut xs = vec![0.0, w * 0.5, w];
    let mut ys = vec![0.0, h * 0.5, h];
    for i in 0..inst.nodes.len() {
        if moving[i] {
            continue;
        }
        let n = &inst.nodes[i];
        if n.fill.is_none() && matches!(n.content, RenderContent::Box) {
            continue; // structure draws nothing; aligning to it looks random
        }
        let hb = {
            let a = &world[i];
            let r = frame.get(i).and_then(|f| f.rect).unwrap_or(n.rect);
            let pts = [
                apply(a, [r.x, r.y]),
                apply(a, [r.x + r.w, r.y]),
                apply(a, [r.x + r.w, r.y + r.h]),
                apply(a, [r.x, r.y + r.h]),
            ];
            let px = pts.map(|p| p[0]);
            let py = pts.map(|p| p[1]);
            [
                px.iter().copied().fold(f32::MAX, f32::min),
                py.iter().copied().fold(f32::MAX, f32::min),
                px.iter().copied().fold(f32::MIN, f32::max),
                py.iter().copied().fold(f32::MIN, f32::max),
            ]
        };
        xs.extend([hb[0], (hb[0] + hb[2]) * 0.5, hb[2]]);
        ys.extend([hb[1], (hb[1] + hb[3]) * 0.5, hb[3]]);
    }
    (xs, ys)
}

/// The best candidate within threshold for any of `edges`, as (adjustment,
/// guide position).
fn best_snap(edges: &[f32], candidates: &[f32]) -> Option<(f32, f32)> {
    let mut best: Option<(f32, f32)> = None;
    for &e in edges {
        for &c in candidates {
            let d = c - e;
            if d.abs() <= SNAP_THRESHOLD && best.is_none_or(|(bd, _)| d.abs() < bd.abs()) {
                best = Some((d, c));
            }
        }
    }
    best
}

pub fn drag_to(
    inst: &SceneInstance,
    frame: &[FrameNode],
    drag: &Drag,
    x: f32,
    y: f32,
    snap: bool,
) -> DragResult {
    let (cand_x, cand_y) = if snap {
        snap_candidates(inst, frame, drag.node)
    } else {
        (Vec::new(), Vec::new())
    };
    let raw_dx = x - drag.start[0];
    let raw_dy = y - drag.start[1];
    let [bx, by, bw, bh] = drag.orig_bbox;
    let mut guides = Vec::new();

    if drag.handle == Handle::Move {
        let mut dx = raw_dx;
        let mut dy = raw_dy;
        if let Some((adj, at)) = best_snap(&[bx + dx, bx + bw * 0.5 + dx, bx + bw + dx], &cand_x) {
            dx += adj;
            guides.push(Guide { axis: 0, at });
        }
        if let Some((adj, at)) = best_snap(&[by + dy, by + bh * 0.5 + dy, by + bh + dy], &cand_y) {
            dy += adj;
            guides.push(Guide { axis: 1, at });
        }
        // Rounded: authored positions are whole pixels. Sub-pixel placement is
        // the renderer's job during motion, not a thing to write into a
        // document a person then has to read.
        let (dx, dy) = (dx.round(), dy.round());
        return DragResult {
            rect: [bx + dx, by + dy, bw, bh],
            dx,
            dy,
            w: drag.orig_size[0],
            h: drag.orig_size[1],
            guides,
        };
    }

    // Resize: move only the grabbed edges of the bbox, snap them, then map the
    // bbox change back to the node's own units through the world scale.
    let (mut x0, mut y0, mut x1, mut y1) = (bx, by, bx + bw, by + bh);
    if drag.handle.moves_left() {
        x0 += raw_dx;
        if let Some((adj, at)) = best_snap(&[x0], &cand_x) {
            x0 += adj;
            guides.push(Guide { axis: 0, at });
        }
    }
    if drag.handle.moves_right() {
        x1 += raw_dx;
        if let Some((adj, at)) = best_snap(&[x1], &cand_x) {
            x1 += adj;
            guides.push(Guide { axis: 0, at });
        }
    }
    if drag.handle.moves_top() {
        y0 += raw_dy;
        if let Some((adj, at)) = best_snap(&[y0], &cand_y) {
            y0 += adj;
            guides.push(Guide { axis: 1, at });
        }
    }
    if drag.handle.moves_bottom() {
        y1 += raw_dy;
        if let Some((adj, at)) = best_snap(&[y1], &cand_y) {
            y1 += adj;
            guides.push(Guide { axis: 1, at });
        }
    }

    // A box no smaller than a pixel: a zero-size node cannot be grabbed again.
    let min_w = drag.scale[0].max(1e-3);
    let min_h = drag.scale[1].max(1e-3);
    if x1 - x0 < min_w {
        if drag.handle.moves_left() {
            x0 = x1 - min_w;
        } else {
            x1 = x0 + min_w;
        }
    }
    if y1 - y0 < min_h {
        if drag.handle.moves_top() {
            y0 = y1 - min_h;
        } else {
            y1 = y0 + min_h;
        }
    }

    let w = ((x1 - x0) / drag.scale[0].max(1e-3)).round().max(1.0);
    let h = ((y1 - y0) / drag.scale[1].max(1e-3)).round().max(1.0);
    DragResult {
        rect: [x0, y0, x1 - x0, y1 - y0],
        dx: 0.0,
        dy: 0.0,
        w,
        h,
        guides,
    }
}

// ---------------------------------------------------------------------------
// Applying an edit to the document
// ---------------------------------------------------------------------------

fn find_node_mut<'a>(n: &'a mut Node, id: &str) -> Option<&'a mut Node> {
    if n.id == id {
        return Some(n);
    }
    n.children.iter_mut().find_map(|c| find_node_mut(c, id))
}

/// Move a node by editing its margins — the only position the format has.
///
/// For a `position: absolute` node this is free placement: it is out of flow,
/// margins offset it from its static position, and nothing else moves. For a
/// relative node it adjusts spacing within the flex flow — siblings WILL
/// shift, which is flexbox telling the truth. The inspector offers the
/// position switch for designers who want the pin.
pub fn apply_move(doc: &mut SceneDoc, id: &str, dx: f32, dy: f32) -> Result<(), String> {
    let n = find_node_mut(&mut doc.root, id).ok_or_else(|| format!("no node {id}"))?;
    n.layout.margin.left += dx;
    n.layout.margin.top += dy;
    Ok(())
}

/// Fix a node's size in pixels.
///
/// A `Content`-sized node becomes fixed — the designer just said so by
/// dragging a handle, and keeping the drag while silently discarding its
/// result would be worse. Growing back to content sizing is one edit in the
/// inspector.
pub fn apply_resize(doc: &mut SceneDoc, id: &str, w: f32, h: f32) -> Result<(), String> {
    let n = find_node_mut(&mut doc.root, id).ok_or_else(|| format!("no node {id}"))?;
    n.layout.width = Dimension::Px(w.max(1.0));
    n.layout.height = Dimension::Px(h.max(1.0));
    Ok(())
}

/// What the inspector shows for one node: the AUTHORED values, straight from
/// the document — not the solved geometry, which is [`handles_for`]'s answer.
/// The two are different facts and the inspector shows both.
#[derive(Debug, Clone, Serialize)]
pub struct NodeFacts {
    pub id: String,
    /// As authored: encoded straight RGBA, `None` for an unfilled box.
    /// For a text node this IS the text colour.
    pub fill: Option<[f32; 4]>,
    pub radius: f32,
    pub opacity: f32,
    /// The pin: an absolute node moves freely; a relative one lives in the
    /// flex flow.
    pub absolute: bool,
    pub kind: String,
    /// Contents masked to this node's bounds.
    pub clip: bool,
    /// How each axis is authored: "px" (fixed) or "content" (hugs what is
    /// inside). The SOLVED size is the handles' answer; this is the setting.
    pub width_mode: String,
    pub height_mode: String,
    /// Present exactly when the node is text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<TextFacts>,
    /// Present exactly when the node draws a picture.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<ImageFacts>,
    /// Authored spacing, `[top, right, bottom, left]` — margin places the
    /// node, padding keeps contents off a container's edge.
    pub margin: [f32; 4],
    pub padding: [f32; 4],
    /// Present exactly when the node plays a clip.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video: Option<VideoFacts>,
    /// The outline, when there is one: encoded straight RGBA plus width.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stroke: Option<StrokeFacts>,
    /// The drop shadow, when there is one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shadow: Option<ShadowFacts>,
}

/// What the inspector shows for a clip-playing node.
#[derive(Debug, Clone, Serialize)]
pub struct VideoFacts {
    /// The field whose value picks the clip, when field-driven.
    pub field: Option<String>,
    pub fit: String,
    pub looping: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct StrokeFacts {
    pub color: [f32; 4],
    pub width: f32,
}

#[derive(Debug, Clone, Serialize)]
pub struct ShadowFacts {
    pub color: [f32; 4],
    pub dx: f32,
    pub dy: f32,
    pub blur: f32,
}

/// What the inspector shows for a node drawing a picture.
#[derive(Debug, Clone, Serialize)]
pub struct ImageFacts {
    pub asset_key: String,
    /// The content hash behind the key, when the document declares it — what
    /// an asset picker actually selects by.
    pub hash: Option<String>,
    pub fit: String,
}

/// What the inspector shows for a text node, beyond what every node has.
#[derive(Debug, Clone, Serialize)]
pub struct TextFacts {
    /// The binding expression — a literal, or `${field}` references, or both.
    pub expr: String,
    pub family: String,
    pub weight: u16,
    pub size: f32,
    pub align: String,
}

fn find_node<'a>(n: &'a Node, id: &str) -> Option<&'a Node> {
    if n.id == id {
        return Some(n);
    }
    n.children.iter().find_map(|c| find_node(c, id))
}

pub fn node_facts(doc: &SceneDoc, id: &str) -> Result<NodeFacts, String> {
    let n = find_node(&doc.root, id).ok_or_else(|| format!("no node {id}"))?;
    let text = match &n.content {
        Some(pmg_scene::doc::Content::Text {
            binding,
            font,
            align,
            ..
        }) => Some(TextFacts {
            expr: binding.expr.clone(),
            family: font.family.clone(),
            weight: font.weight,
            size: font.size,
            align: format!("{align:?}").to_lowercase(),
        }),
        _ => None,
    };
    let image = match &n.content {
        Some(pmg_scene::doc::Content::Image { asset_key, fit, .. }) => Some(ImageFacts {
            asset_key: asset_key.clone(),
            hash: doc
                .assets
                .iter()
                .find(|a| &a.key == asset_key)
                .map(|a| a.hash.clone()),
            fit: format!("{fit:?}").to_lowercase(),
        }),
        _ => None,
    };
    let video = match &n.content {
        Some(pmg_scene::doc::Content::Video {
            fit,
            looping,
            field,
            ..
        }) => Some(VideoFacts {
            field: field.clone(),
            fit: format!("{fit:?}").to_lowercase(),
            looping: *looping,
        }),
        _ => None,
    };
    let mode = |d: &Dimension| match d {
        Dimension::Px(_) => "px".to_string(),
        _ => "content".to_string(),
    };
    Ok(NodeFacts {
        id: n.id.clone(),
        fill: n.style.fill.map(|c| c.rgba),
        radius: n.style.radius,
        opacity: n.style.opacity,
        absolute: n.layout.position == Position::Absolute,
        kind: format!("{:?}", n.kind).to_lowercase(),
        clip: n.clip,
        width_mode: mode(&n.layout.width),
        height_mode: mode(&n.layout.height),
        margin: [
            n.layout.margin.top,
            n.layout.margin.right,
            n.layout.margin.bottom,
            n.layout.margin.left,
        ],
        padding: [
            n.layout.padding.top,
            n.layout.padding.right,
            n.layout.padding.bottom,
            n.layout.padding.left,
        ],
        text,
        image,
        video,
        stroke: n.style.stroke.map(|s| StrokeFacts {
            color: s.color.rgba,
            width: s.width,
        }),
        shadow: n.style.shadow.map(|s| ShadowFacts {
            color: s.color.rgba,
            dx: s.dx,
            dy: s.dy,
            blur: s.blur,
        }),
    })
}

/// Set or clear the fill. Clearing makes a box structural — invisible and
/// transparent to clicks — which is what "no fill" honestly means here, and
/// the inspector says so before the designer loses the selection.
pub fn apply_fill(doc: &mut SceneDoc, id: &str, fill: Option<[f32; 4]>) -> Result<(), String> {
    let n = find_node_mut(&mut doc.root, id).ok_or_else(|| format!("no node {id}"))?;
    n.style.fill = fill.map(|rgba| pmg_scene::doc::Rgba { rgba });
    Ok(())
}

pub fn apply_radius(doc: &mut SceneDoc, id: &str, radius: f32) -> Result<(), String> {
    let n = find_node_mut(&mut doc.root, id).ok_or_else(|| format!("no node {id}"))?;
    // Non-negative; the renderer clamps to the half-extent at draw, so an
    // over-large radius is a capsule rather than an error.
    n.style.radius = radius.max(0.0);
    Ok(())
}

pub fn apply_opacity(doc: &mut SceneDoc, id: &str, opacity: f32) -> Result<(), String> {
    let n = find_node_mut(&mut doc.root, id).ok_or_else(|| format!("no node {id}"))?;
    n.style.opacity = opacity.clamp(0.0, 1.0);
    Ok(())
}

/// Pin or unpin: absolute takes the node out of flow (margins become free
/// placement); relative returns it to the flexbox, where its siblings take
/// their positions from it again.
pub fn apply_position(doc: &mut SceneDoc, id: &str, absolute: bool) -> Result<(), String> {
    let n = find_node_mut(&mut doc.root, id).ok_or_else(|| format!("no node {id}"))?;
    n.layout.position = if absolute {
        Position::Absolute
    } else {
        Position::Relative
    };
    Ok(())
}

// ---------------------------------------------------------------------------
// Timeline edits
// ---------------------------------------------------------------------------

/// The properties a track may animate — exactly the set `apply_tracks`
/// honours. A key on anything else would author silence: the renderer drops
/// unknown targets, so the whitelist is enforced at the point of the edit
/// rather than discovered as a key that never moves anything. `layout.*` is
/// deliberately absent (§3.2: animating layout relayouts every frame).
pub const ANIMATABLE: &[&str] = &[
    "transform.translate.x",
    "transform.translate.y",
    "transform.rotate",
    "transform.scale.x",
    "transform.scale.y",
    "style.opacity",
];

fn timeline_mut<'a>(
    doc: &'a mut SceneDoc,
    name: &str,
) -> Result<&'a mut pmg_scene::doc::Timeline, String> {
    doc.timelines
        .iter_mut()
        .find(|t| t.name == name)
        .ok_or_else(|| format!("no timeline named {name}"))
}

pub fn apply_add_timeline(doc: &mut SceneDoc, name: &str, duration_ms: f32) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("a timeline needs a name".into());
    }
    if doc.timelines.iter().any(|t| t.name == name) {
        return Err(format!("there is already a timeline named {name}"));
    }
    doc.timelines.push(pmg_scene::doc::Timeline {
        name: name.into(),
        duration_ms: duration_ms.max(1.0),
        on_complete: Default::default(),
        looping: false,
        tracks: vec![],
    });
    Ok(())
}

pub fn apply_delete_timeline(doc: &mut SceneDoc, name: &str) -> Result<(), String> {
    let before = doc.timelines.len();
    doc.timelines.retain(|t| t.name != name);
    if doc.timelines.len() == before {
        return Err(format!("no timeline named {name}"));
    }
    Ok(())
}

pub fn apply_set_timeline_duration(
    doc: &mut SceneDoc,
    name: &str,
    duration_ms: f32,
) -> Result<(), String> {
    timeline_mut(doc, name)?.duration_ms = duration_ms.max(1.0);
    Ok(())
}

/// Two keys closer than this are the same key: editing a key's value must
/// replace it, not stack a twin the evaluator then steps through.
const KEY_EPSILON_MS: f32 = 0.5;

/// Set (insert or replace) a key. Keys stay sorted by time — `pmg_anim::eval`
/// requires it, and an editor that let them drift out of order would author
/// timelines that evaluate wrong with nothing on screen saying why.
pub fn apply_set_key(
    doc: &mut SceneDoc,
    timeline: &str,
    node_id: &str,
    prop: &str,
    t: f32,
    v: f32,
    ease: pmg_anim::Ease,
) -> Result<(), String> {
    if !ANIMATABLE.contains(&prop) {
        return Err(format!(
            "{prop} cannot be animated. Animate transforms and opacity; layout is solved once, at arm"
        ));
    }
    if find_node(&doc.root, node_id).is_none() {
        return Err(format!("no node {node_id}"));
    }
    let t = t.max(0.0);
    let target = format!("{node_id}/{prop}");
    let tl = timeline_mut(doc, timeline)?;
    let track = match tl.tracks.iter_mut().find(|tr| tr.target == target) {
        Some(tr) => tr,
        None => {
            tl.tracks.push(pmg_scene::doc::Track {
                target,
                blend: Default::default(),
                keys: vec![],
            });
            tl.tracks.last_mut().expect("just pushed")
        }
    };
    track.keys.retain(|k| (k.t - t).abs() > KEY_EPSILON_MS);
    let at = track.keys.partition_point(|k| k.t < t);
    track.keys.insert(at, pmg_anim::Key { t, v, ease });
    Ok(())
}

/// Delete the key at `t`. An emptied track goes with it — a track with no
/// keys is not a quieter track, it is clutter the evaluator skips.
pub fn apply_delete_key(
    doc: &mut SceneDoc,
    timeline: &str,
    node_id: &str,
    prop: &str,
    t: f32,
) -> Result<(), String> {
    let target = format!("{node_id}/{prop}");
    let tl = timeline_mut(doc, timeline)?;
    let track = tl
        .tracks
        .iter_mut()
        .find(|tr| tr.target == target)
        .ok_or_else(|| format!("no track for {target}"))?;
    let before = track.keys.len();
    track.keys.retain(|k| (k.t - t).abs() > KEY_EPSILON_MS);
    if track.keys.len() == before {
        return Err(format!("no key at {t} ms on {target}"));
    }
    tl.tracks.retain(|tr| !tr.keys.is_empty());
    Ok(())
}

fn id_taken(n: &Node, id: &str) -> bool {
    n.id == id || n.children.iter().any(|c| id_taken(c, id))
}

/// The first free id of the form `<prefix>-N`. Ids are how everything
/// addresses a node — timelines, `visible_if`, this very editor — so a
/// collision is not a cosmetic problem, and the counter walks until it finds
/// air.
fn fresh_id(doc: &SceneDoc, prefix: &str) -> String {
    (1..)
        .map(|n| format!("{prefix}-{n}"))
        .find(|id| !id_taken(&doc.root, id))
        .expect("the integers do not run out")
}

/// Add a box under `parent` (the root when `None`) and return its id.
///
/// It arrives visible and grabbable — filled white, fixed 240×120 — and
/// RELATIVE: a template is a responsive layout first, and a new box takes its
/// place in the parent's flow like everything else. Free placement is one pin
/// away in the inspector, which is a decision the designer makes, not one the
/// editor smuggles in.
pub fn apply_add_box(doc: &mut SceneDoc, parent: Option<&str>) -> Result<String, String> {
    let id = fresh_id(doc, "box");
    let parent_id = parent.unwrap_or(&doc.root.id).to_string();
    let p =
        find_node_mut(&mut doc.root, &parent_id).ok_or_else(|| format!("no node {parent_id}"))?;
    p.children.push(Node {
        id: id.clone(),
        display_name: None,
        kind: pmg_scene::doc::NodeKind::Box,
        layout: pmg_scene::doc::Layout {
            width: Dimension::Px(240.0),
            height: Dimension::Px(120.0),
            ..Default::default()
        },
        transform: Default::default(),
        style: pmg_scene::doc::Style {
            fill: Some(pmg_scene::doc::Rgba {
                rgba: [1.0, 1.0, 1.0, 1.0],
            }),
            ..Default::default()
        },
        clip: false,
        visible_if: None,
        content: None,
        children: vec![],
        unknown: Default::default(),
    });
    Ok(id)
}

/// Add a text node under `parent` (the root when `None`) and return its id.
///
/// It arrives saying "Text" in white at 48 px, Content-sized — text measures
/// itself, that is the responsive premise — and in the family the document
/// already declares. A document with no fonts yet gets a `Default` family
/// declared here: arming will then say, in a sentence, that a font must be
/// added — which is the honest failure, found now and not on a booth Mac.
pub fn apply_add_text(doc: &mut SceneDoc, parent: Option<&str>) -> Result<String, String> {
    use pmg_scene::doc::{Align, Binding, Content, FontRef, FontSpec, NodeKind};

    let (family, weight) = match doc.fonts.first() {
        Some(f) => (f.family.clone(), f.weight),
        None => {
            doc.fonts.push(FontRef {
                family: "Default".into(),
                weight: 400,
                style: None,
                asset: None,
                system_fallback: false,
            });
            ("Default".into(), 400)
        }
    };

    let id = fresh_id(doc, "text");
    let parent_id = parent.unwrap_or(&doc.root.id).to_string();
    let p =
        find_node_mut(&mut doc.root, &parent_id).ok_or_else(|| format!("no node {parent_id}"))?;
    p.children.push(Node {
        id: id.clone(),
        display_name: None,
        kind: NodeKind::Text,
        layout: pmg_scene::doc::Layout {
            width: Dimension::Content,
            height: Dimension::Content,
            ..Default::default()
        },
        transform: Default::default(),
        style: pmg_scene::doc::Style {
            fill: Some(pmg_scene::doc::Rgba {
                rgba: [1.0, 1.0, 1.0, 1.0],
            }),
            ..Default::default()
        },
        clip: false,
        visible_if: None,
        content: Some(Content::Text {
            binding: Binding {
                expr: "Text".into(),
                transform: vec![],
            },
            font: FontSpec {
                family,
                weight,
                size: 48.0,
                features: vec!["kern".into(), "liga".into()],
                tracking: 0.0,
                line_height: 1.2,
            },
            align: Align::Start,
            max_lines: None,
        }),
        children: vec![],
        unknown: Default::default(),
    });
    Ok(id)
}

/// Move a key to a new time, in ONE document operation.
///
/// Not delete-then-set composed by the caller: session editors each clone the
/// armed document, so two calls in one gesture would both edit the same base
/// and the second would silently discard the first — the retime would
/// DUPLICATE the key instead of moving it. One operation, one truth.
pub fn apply_retime_key(
    doc: &mut SceneDoc,
    timeline: &str,
    node_id: &str,
    prop: &str,
    from: f32,
    to: f32,
) -> Result<(), String> {
    // Read the key's value and ease first, so the move carries them.
    let target = format!("{node_id}/{prop}");
    let (v, ease) = {
        let tl = doc
            .timelines
            .iter()
            .find(|t| t.name == timeline)
            .ok_or_else(|| format!("no timeline {timeline}"))?;
        let track = tl
            .tracks
            .iter()
            .find(|tr| tr.target == target)
            .ok_or_else(|| format!("no {prop} keys on {node_id} in {timeline}"))?;
        let key = track
            .keys
            .iter()
            .find(|k| (k.t - from).abs() <= KEY_EPSILON_MS)
            .ok_or_else(|| format!("no key at {from} ms"))?;
        (key.v, key.ease)
    };
    apply_delete_key(doc, timeline, node_id, prop, from)?;
    apply_set_key(doc, timeline, node_id, prop, to, v, ease)
}

/// Set one spacing edge numerically: `kind` is "margin" or "padding", `edge`
/// is top/right/bottom/left. Margins place a node (drag edits them already —
/// this is the same fact, typed); padding is the room a container keeps
/// between its edge and its contents, which is how text gets breathing space
/// inside a band.
pub fn apply_set_spacing(
    doc: &mut SceneDoc,
    id: &str,
    kind: &str,
    edge: &str,
    v: f32,
) -> Result<(), String> {
    let n = find_node_mut(&mut doc.root, id).ok_or_else(|| format!("no node {id}"))?;
    let e = match kind {
        "margin" => &mut n.layout.margin,
        "padding" => &mut n.layout.padding,
        other => return Err(format!("no spacing named {other}")),
    };
    match edge {
        "top" => e.top = v,
        "right" => e.right = v,
        "bottom" => e.bottom = v,
        "left" => e.left = v,
        other => return Err(format!("no edge named {other}")),
    }
    Ok(())
}

/// Set or clear a node's outline. The stroke draws INSIDE the edge — it never
/// grows the node past the box layout gave it, so adding one cannot move a
/// neighbour or change what a clip reveals.
pub fn apply_set_stroke(
    doc: &mut SceneDoc,
    id: &str,
    stroke: Option<([f32; 4], f32)>,
) -> Result<(), String> {
    let n = find_node_mut(&mut doc.root, id).ok_or_else(|| format!("no node {id}"))?;
    n.style.stroke = stroke.map(|(rgba, width)| pmg_scene::doc::Stroke {
        color: pmg_scene::doc::Rgba { rgba },
        width: width.max(0.5),
    });
    Ok(())
}

/// Set or clear a node's drop shadow: a soft lobe of its own silhouette,
/// offset by `dx, dy`, its edge `blur` design units wide. Drawn under the
/// node; never part of layout.
pub fn apply_set_shadow(
    doc: &mut SceneDoc,
    id: &str,
    shadow: Option<([f32; 4], f32, f32, f32)>,
) -> Result<(), String> {
    let n = find_node_mut(&mut doc.root, id).ok_or_else(|| format!("no node {id}"))?;
    n.style.shadow = shadow.map(|(rgba, dx, dy, blur)| pmg_scene::doc::Shadow {
        color: pmg_scene::doc::Rgba { rgba },
        dx,
        dy,
        blur: blur.max(0.0),
    });
    Ok(())
}

/// Clip a node's contents to its bounds — the mask that makes text "pop out"
/// of a band by NOT being visible until it enters. The scissor is axis-aligned
/// (the renderer's documented limit): a rotated container clips to its
/// axis-aligned bounds, not its rotated silhouette.
pub fn apply_set_clip(doc: &mut SceneDoc, id: &str, clip: bool) -> Result<(), String> {
    let n = find_node_mut(&mut doc.root, id).ok_or_else(|| format!("no node {id}"))?;
    n.clip = clip;
    Ok(())
}

/// Set one axis to a fixed size or to content sizing.
///
/// `None` means **fit content**: the axis hugs what is inside — a band that
/// grows with its name field is `width: Content` plus padding, which is the
/// responsive premise (ADR 0005) doing its job. Per axis, so a lower third can
/// hug its text horizontally while holding the height the design fixed.
pub fn apply_set_dimension(
    doc: &mut SceneDoc,
    id: &str,
    axis: &str,
    px: Option<f32>,
) -> Result<(), String> {
    let n = find_node_mut(&mut doc.root, id).ok_or_else(|| format!("no node {id}"))?;
    let dim = match px {
        Some(v) => Dimension::Px(v.max(1.0)),
        None => Dimension::Content,
    };
    match axis {
        "width" => n.layout.width = dim,
        "height" => n.layout.height = dim,
        other => return Err(format!("no axis named {other}")),
    }
    Ok(())
}

/// Every node whose content or visibility references `key`, by id — the
/// answer a refusal needs in its sentence.
fn field_users(n: &Node, key: &str, out: &mut Vec<String>) {
    let uses = match &n.content {
        Some(pmg_scene::doc::Content::Text { binding, .. }) => {
            pmg_scene::bind::referenced_fields(&binding.expr)
                .iter()
                .any(|k| k.as_str() == key)
        }
        Some(
            pmg_scene::doc::Content::Image { field: Some(f), .. }
            | pmg_scene::doc::Content::Video { field: Some(f), .. },
        ) => f == key,
        _ => false,
    } || n.visible_if.as_ref().is_some_and(|v| v.field == key);
    if uses {
        out.push(n.id.clone());
    }
    for c in &n.children {
        field_users(c, key, out);
    }
}

/// Declare a field. The KEY is the contract — Items bind by it, bindings
/// reference it — so it must be new; label and sample start as the key and are
/// the editable half.
pub fn apply_add_field(doc: &mut SceneDoc, key: &str) -> Result<(), String> {
    use pmg_scene::doc::{Field, FieldType};
    let key = key.trim();
    if key.is_empty() {
        return Err("a field needs a key".into());
    }
    if doc.fields.iter().any(|f| f.key == key) {
        return Err(format!("there is already a field named {key}"));
    }
    doc.fields.push(Field {
        key: key.to_string(),
        ty: FieldType::Text,
        label: key.to_string(),
        required: false,
        max_len: None,
        sample: Some(key.to_string()),
    });
    Ok(())
}

/// Edit a field's presentation: label, sample, required. Never the key — the
/// key is what Items and bindings address, and renaming it would orphan both
/// silently. A different key is a different field.
pub fn apply_update_field(
    doc: &mut SceneDoc,
    key: &str,
    label: &str,
    sample: Option<&str>,
    required: bool,
) -> Result<(), String> {
    let f = doc
        .fields
        .iter_mut()
        .find(|f| f.key == key)
        .ok_or_else(|| format!("no field {key}"))?;
    let label = label.trim();
    f.label = if label.is_empty() {
        key.to_string()
    } else {
        label.to_string()
    };
    f.sample = sample
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from);
    f.required = required;
    Ok(())
}

/// Remove a field — refused, with the node ids in the sentence, while anything
/// still references it. Compile would fail on the dangling reference anyway;
/// this refusal happens while the designer is looking at the reason.
pub fn apply_delete_field(doc: &mut SceneDoc, key: &str) -> Result<(), String> {
    if !doc.fields.iter().any(|f| f.key == key) {
        return Err(format!("no field {key}"));
    }
    let mut users = Vec::new();
    field_users(&doc.root, key, &mut users);
    if !users.is_empty() {
        return Err(format!(
            "{key} is still used by {} — remove those references first",
            users.join(", ")
        ));
    }
    doc.fields.retain(|f| f.key != key);
    Ok(())
}

/// The asset key for `hash` in this document, declaring an [`AssetRef`] when
/// the hash is new. One hash, one key: assigning the same picture twice reuses
/// the declaration rather than stacking twins.
fn image_asset_key(doc: &mut SceneDoc, hash: &str) -> String {
    if let Some(a) = doc.assets.iter().find(|a| a.hash == hash) {
        return a.key.clone();
    }
    let key = (1..)
        .map(|n| format!("image-{n}"))
        .find(|k| !doc.assets.iter().any(|a| &a.key == k))
        .expect("the integers do not run out");
    doc.assets.push(pmg_scene::doc::AssetRef {
        key: key.clone(),
        hash: hash.to_string(),
        kind: "image".into(),
    });
    key
}

/// Add an image node under `parent` (the root when `None`) and return its id.
///
/// It arrives 320×180, scale-to-fill, showing the asset `hash` names — which
/// is declared in `doc.assets` on the way in. The size is fixed rather than
/// content-sized on purpose: an image has no one honest size, and a designer
/// who wants the native aspect gets it by picking a fit that preserves it.
pub fn apply_add_image(
    doc: &mut SceneDoc,
    parent: Option<&str>,
    hash: &str,
) -> Result<String, String> {
    use pmg_scene::doc::{Content, Fit, NodeKind};

    let key = image_asset_key(doc, hash);
    let id = fresh_id(doc, "image");
    let parent_id = parent.unwrap_or(&doc.root.id).to_string();
    let p =
        find_node_mut(&mut doc.root, &parent_id).ok_or_else(|| format!("no node {parent_id}"))?;
    p.children.push(Node {
        id: id.clone(),
        display_name: None,
        kind: NodeKind::Image,
        layout: pmg_scene::doc::Layout {
            width: Dimension::Px(320.0),
            height: Dimension::Px(180.0),
            ..Default::default()
        },
        transform: Default::default(),
        style: Default::default(),
        clip: false,
        visible_if: None,
        content: Some(Content::Image {
            asset_key: key,
            fit: Fit::Cover,
            field: None,
        }),
        children: vec![],
        unknown: Default::default(),
    });
    Ok(id)
}

/// Add a video node under `parent` (the root when `None`) and return its id.
///
/// It arrives FULL-CANVAS and field-driven: a fresh video-type field is
/// declared (`clip`, then `clip-N`) and the node plays whatever content
/// address the Item's data puts there. This is the "one template, every
/// clip" shape — the template says a video goes here, the Item says which.
/// On the designer's canvas it shows as a dark plate: the browser cannot
/// decode broadcast codecs, and pretending otherwise would preview a lie.
pub fn apply_add_video(doc: &mut SceneDoc, parent: Option<&str>) -> Result<String, String> {
    use pmg_scene::doc::{Content, Field, FieldType, Fit, NodeKind};

    let field_key = (0..)
        .map(|n| {
            if n == 0 {
                "clip".to_string()
            } else {
                format!("clip-{n}")
            }
        })
        .find(|k| !doc.fields.iter().any(|f| &f.key == k))
        .expect("the integers do not run out");
    doc.fields.push(Field {
        key: field_key.clone(),
        ty: FieldType::Video,
        label: field_key.clone(),
        required: false,
        max_len: None,
        sample: None,
    });

    let id = fresh_id(doc, "video");
    let (cw, ch) = (doc.canvas.width as f32, doc.canvas.height as f32);
    let parent_id = parent.unwrap_or(&doc.root.id).to_string();
    let p =
        find_node_mut(&mut doc.root, &parent_id).ok_or_else(|| format!("no node {parent_id}"))?;
    p.children.push(Node {
        id: id.clone(),
        display_name: None,
        kind: NodeKind::Video,
        layout: pmg_scene::doc::Layout {
            width: Dimension::Px(cw),
            height: Dimension::Px(ch),
            position: Position::Absolute,
            ..Default::default()
        },
        transform: Default::default(),
        style: Default::default(),
        clip: false,
        visible_if: None,
        content: Some(Content::Video {
            asset_key: String::new(),
            fit: Fit::Cover,
            looping: false,
            field: Some(field_key),
        }),
        children: vec![],
        unknown: Default::default(),
    });
    Ok(id)
}

/// Loop a clip, or hold its last frame at the end — holding is the default,
/// because going transparent mid-shot is a graphic vanishing without anyone
/// taking it out.
pub fn apply_set_loop(doc: &mut SceneDoc, id: &str, looping: bool) -> Result<(), String> {
    use pmg_scene::doc::Content;
    let n = find_node_mut(&mut doc.root, id).ok_or_else(|| format!("no node {id}"))?;
    match &mut n.content {
        Some(Content::Video { looping: l, .. }) => {
            *l = looping;
            Ok(())
        }
        _ => Err(format!("{id} has no clip to loop")),
    }
}

/// Give a node an image, or swap the one it has.
///
/// On a box this is the background-picture move: the image draws in the box's
/// rect — rounded by its radius, fitted by its fit — and the box's children
/// draw over it. On an image node it swaps the picture. Text keeps its words:
/// a node cannot serve two contents, and the refusal says so.
pub fn apply_set_image(doc: &mut SceneDoc, id: &str, hash: &str) -> Result<(), String> {
    use pmg_scene::doc::{Content, Fit};

    let key = image_asset_key(doc, hash);
    let n = find_node_mut(&mut doc.root, id).ok_or_else(|| format!("no node {id}"))?;
    match &mut n.content {
        Some(Content::Image { asset_key, .. }) => {
            *asset_key = key;
            Ok(())
        }
        None => {
            n.content = Some(Content::Image {
                asset_key: key,
                fit: Fit::Cover,
                field: None,
            });
            Ok(())
        }
        Some(_) => Err(format!(
            "{id} already has content that is not an image — a node draws one thing"
        )),
    }
}

/// Take a box's background image away, returning it to its fill. Refused on
/// an image node: that node IS its picture — delete the node instead.
pub fn apply_clear_image(doc: &mut SceneDoc, id: &str) -> Result<(), String> {
    use pmg_scene::doc::{Content, NodeKind};

    let n = find_node_mut(&mut doc.root, id).ok_or_else(|| format!("no node {id}"))?;
    if !matches!(n.content, Some(Content::Image { .. })) {
        return Err(format!("{id} has no image to remove"));
    }
    if n.kind == NodeKind::Image {
        return Err(format!(
            "{id} is an image node — deleting the node is how it goes away"
        ));
    }
    n.content = None;
    Ok(())
}

/// Set how a picture meets a box it disagrees with: scale to fill (crops),
/// scale to fit (letterboxes), or stretch (distorts). Applies to stills and
/// clips alike — a clip is an image whose texture changes.
pub fn apply_set_fit(doc: &mut SceneDoc, id: &str, fit: &str) -> Result<(), String> {
    use pmg_scene::doc::{Content, Fit};

    let parsed = match fit {
        "cover" => Fit::Cover,
        "contain" => Fit::Contain,
        "stretch" => Fit::Stretch,
        other => return Err(format!("no fit named {other}")),
    };
    let n = find_node_mut(&mut doc.root, id).ok_or_else(|| format!("no node {id}"))?;
    match &mut n.content {
        Some(Content::Image { fit, .. }) | Some(Content::Video { fit, .. }) => {
            *fit = parsed;
            Ok(())
        }
        _ => Err(format!("{id} has no picture to fit")),
    }
}

/// The mutable text content of a node, or a sentence about why not.
fn text_content_mut<'a>(
    doc: &'a mut SceneDoc,
    id: &str,
) -> Result<
    (
        &'a mut pmg_scene::doc::Binding,
        &'a mut pmg_scene::doc::FontSpec,
        &'a mut pmg_scene::doc::Align,
    ),
    String,
> {
    let n = find_node_mut(&mut doc.root, id).ok_or_else(|| format!("no node {id}"))?;
    match n.content.as_mut() {
        Some(pmg_scene::doc::Content::Text {
            binding,
            font,
            align,
            ..
        }) => Ok((binding, font, align)),
        _ => Err(format!("{id} is not a text node")),
    }
}

/// Set what a text node says: a literal, `${field}` references, or both.
///
/// Every field the expression references is DECLARED if it is not already —
/// with the key as its label and sample, ready to rename in a fields editor
/// later. Compile refuses an undeclared reference (it is almost always a
/// typo), so the editor that introduces the reference carries the declaration;
/// a designer typing `${speaker}` should get a speaker field, not an error
/// about bookkeeping they were never shown.
pub fn apply_set_text_expr(doc: &mut SceneDoc, id: &str, expr: &str) -> Result<(), String> {
    use pmg_scene::doc::{Field, FieldType};

    let referenced = pmg_scene::bind::referenced_fields(expr);
    let (binding, _, _) = text_content_mut(doc, id)?;
    binding.expr = expr.to_string();
    for key in referenced {
        if !doc.fields.iter().any(|f| f.key == key) {
            doc.fields.push(Field {
                key: key.clone(),
                ty: FieldType::Text,
                label: key.clone(),
                required: false,
                max_len: None,
                sample: Some(key),
            });
        }
    }
    Ok(())
}

/// Set a text node's size in points. Clamped to something visible, the same
/// contract as radius and opacity: a designer typing 0 gets the floor, not an
/// error dialog.
pub fn apply_set_font_size(doc: &mut SceneDoc, id: &str, size: f32) -> Result<(), String> {
    let (_, font, _) = text_content_mut(doc, id)?;
    font.size = size.max(1.0);
    Ok(())
}

/// Set a text node's family and weight, declaring the family in `doc.fonts`
/// when it is new — a family named by content but not declared would fail
/// compile. `asset` is the font file's content hash when the caller knows it
/// (a picker does; typed input may not): carried on the declaration, it is
/// what lets a node stage exactly the bytes the designer chose.
pub fn apply_set_font_family(
    doc: &mut SceneDoc,
    id: &str,
    family: &str,
    weight: u16,
    asset: Option<&str>,
) -> Result<(), String> {
    let family = family.trim();
    if family.is_empty() {
        return Err("a font family needs a name".into());
    }
    {
        let (_, font, _) = text_content_mut(doc, id)?;
        font.family = family.to_string();
        font.weight = weight;
    }
    match doc
        .fonts
        .iter_mut()
        .find(|f| f.family == family && f.weight == weight)
    {
        Some(f) => {
            if let Some(hash) = asset {
                f.asset = Some(hash.to_string());
            }
        }
        None => doc.fonts.push(pmg_scene::doc::FontRef {
            family: family.to_string(),
            weight,
            style: None,
            asset: asset.map(String::from),
            system_fallback: false,
        }),
    }
    Ok(())
}

/// Set a text node's alignment within its box: start, center, or end.
pub fn apply_set_text_align(doc: &mut SceneDoc, id: &str, align: &str) -> Result<(), String> {
    use pmg_scene::doc::Align;
    let parsed = match align {
        "start" => Align::Start,
        "center" => Align::Center,
        "end" => Align::End,
        other => return Err(format!("no alignment named {other}")),
    };
    let (_, _, a) = text_content_mut(doc, id)?;
    *a = parsed;
    Ok(())
}

fn extract_from(n: &mut Node, id: &str) -> Option<Node> {
    if let Some(at) = n.children.iter().position(|c| c.id == id) {
        return Some(n.children.remove(at));
    }
    n.children.iter_mut().find_map(|c| extract_from(c, id))
}

/// Move a node to a new parent at `index` among its children — one operation
/// that is reorder (same parent), reparent (into a container), and un-parent
/// (back out to an ancestor), because a layer tree's drop is all three and a
/// seam between them would be the editor's, not the designer's.
///
/// The node keeps its AUTHORED margins, not its solved position: position here
/// is spacing within a flow, and a new flow answers differently. That is
/// flexbox telling the truth, the same contract as [`apply_move`]. Timeline
/// tracks keep working untouched — they address `id/property`, and the id is
/// the one thing this never changes (§3.1).
pub fn apply_reparent(
    doc: &mut SceneDoc,
    id: &str,
    new_parent: &str,
    index: usize,
) -> Result<(), String> {
    if doc.root.id == id {
        return Err("the root is the canvas itself and cannot be moved".into());
    }
    // Into itself or its own contents would detach the subtree from the
    // document entirely — the tree equivalent of division by zero.
    if let Some(moving) = find_node(&doc.root, id) {
        if find_node(moving, new_parent).is_some() {
            return Err(format!("{new_parent} is inside {id} and cannot contain it"));
        }
    } else {
        return Err(format!("no node {id}"));
    }
    if find_node(&doc.root, new_parent).is_none() {
        return Err(format!("no node {new_parent}"));
    }
    let node = extract_from(&mut doc.root, id).ok_or_else(|| format!("no node {id}"))?;
    let p =
        find_node_mut(&mut doc.root, new_parent).ok_or_else(|| format!("no node {new_parent}"))?;
    // `index` counts positions AFTER the node left its old place — the natural
    // meaning of a drop target — and clamps rather than errors: the caller
    // computed it against a list this removal may just have shortened.
    let at = index.min(p.children.len());
    p.children.insert(at, node);
    Ok(())
}

/// Name a layer for the eye. The `display_name` is presentation only — the id
/// underneath is what timelines and fields address, and renaming can never
/// break either. `None` returns the node to showing its id.
pub fn apply_set_name(doc: &mut SceneDoc, id: &str, name: Option<&str>) -> Result<(), String> {
    let n = find_node_mut(&mut doc.root, id).ok_or_else(|| format!("no node {id}"))?;
    n.display_name = name
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from);
    Ok(())
}

fn remove_from(n: &mut Node, id: &str) -> bool {
    if let Some(at) = n.children.iter().position(|c| c.id == id) {
        n.children.remove(at);
        return true;
    }
    n.children.iter_mut().any(|c| remove_from(c, id))
}

/// Delete a node — and everything inside it, which is what deleting a
/// container means and the interface must say before the press.
///
/// The root is refused: it IS the canvas, and a template with no root is not
/// an empty template, it is not a template.
pub fn apply_delete(doc: &mut SceneDoc, id: &str) -> Result<(), String> {
    if doc.root.id == id {
        return Err("the root is the canvas itself and cannot be deleted".into());
    }
    if remove_from(&mut doc.root, id) {
        Ok(())
    } else {
        Err(format!("no node {id}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::SceneInstance;
    use pmg_scene::doc::Rgba;

    /// A tiny instance built by hand: canvas 400×300, a filled parent at
    /// (50,50) 200×100 with a filled child at (70,60) 40×30 (absolute
    /// coordinates, as the solver reports them).
    fn fixture() -> (SceneInstance, Vec<FrameNode>) {
        use crate::scene::{RenderContent, RenderNode};
        let node = |id: &str, parent, x, y, w, h, fill, radius| RenderNode {
            id: id.into(),
            parent,
            rect: pmg_layout::Rect { x, y, w, h },
            base_transform: Default::default(),
            fill,
            stroke: None,
            shadow: None,
            radius,
            opacity: 1.0,
            clip: false,
            content: RenderContent::Box,
        };
        let white = Some(Rgba { rgba: [1.0; 4] });
        let inst = SceneInstance {
            canvas_w: 400,
            canvas_h: 300,
            nodes: vec![
                node("band", None, 50.0, 50.0, 200.0, 100.0, white, 12.0),
                node("chip", Some(0), 70.0, 60.0, 40.0, 30.0, white, 0.0),
            ],
            ..Default::default()
        };
        let frame = crate::scene::apply_tracks(&inst, &Default::default());
        (inst, frame)
    }

    #[test]
    fn the_topmost_node_wins() {
        let (inst, frame) = fixture();
        // Inside both: the child is painted later, so the child is hit.
        assert_eq!(hit_test(&inst, &frame, 80.0, 70.0), Some(1));
        // Inside only the band.
        assert_eq!(hit_test(&inst, &frame, 200.0, 100.0), Some(0));
        // Outside everything.
        assert_eq!(hit_test(&inst, &frame, 10.0, 10.0), None);
    }

    #[test]
    fn a_rounded_corner_notch_is_a_miss() {
        let (inst, frame) = fixture();
        // The band's corner radius is 12: the square corner (51,51) lies
        // outside the rounded shape. This is the case a JS bbox test gets
        // wrong, and the reason the SDF is shared with the shader.
        assert_eq!(hit_test(&inst, &frame, 51.0, 51.0), None);
        // Just inside the arc is a hit.
        assert_eq!(hit_test(&inst, &frame, 60.0, 60.0), Some(0));
    }

    #[test]
    fn an_unfilled_box_is_transparent_to_clicks() {
        let (mut inst, frame) = fixture();
        inst.nodes[1].fill = None;
        assert_eq!(hit_test(&inst, &frame, 80.0, 70.0), Some(0));
    }

    #[test]
    fn a_translated_node_is_hit_where_it_is_drawn() {
        let (mut inst, _) = fixture();
        inst.nodes[0].base_transform.translate = [100.0, 0.0];
        // The child COMPOSES with the parent: both moved.
        let frame = crate::scene::apply_tracks(&inst, &Default::default());
        assert_eq!(hit_test(&inst, &frame, 80.0, 70.0), None);
        assert_eq!(hit_test(&inst, &frame, 180.0, 70.0), Some(1));
    }

    #[test]
    fn a_rotated_node_reports_rotated_corners() {
        let (mut inst, _) = fixture();
        inst.nodes[0].base_transform.rotate = 90.0;
        let frame = crate::scene::apply_tracks(&inst, &Default::default());
        let h = handles_for(&inst, &frame, 0);
        assert!(h.rotated);
        // 90° about the centre (150,100): the 200×100 band's bbox becomes
        // 100×200 centred at the same point.
        let [x, y, w, hgt] = h.bbox;
        assert!((x - 100.0).abs() < 1e-3 && (y - 0.0).abs() < 1e-3);
        assert!((w - 100.0).abs() < 1e-3 && (hgt - 200.0).abs() < 1e-3);
    }

    #[test]
    fn a_move_snaps_to_a_sibling_edge_and_says_which() {
        let (inst, frame) = fixture();
        // Drag the child right, to 3px short of the band's right edge (250).
        // 110 -> 247: dx = 137; snapped dx = 140.
        let d = begin_drag(&inst, &frame, 1, Handle::Move, 80.0, 70.0);
        let r = drag_to(&inst, &frame, &d, 217.0, 70.0, true);
        assert_eq!(r.dx, 140.0);
        assert!(r.guides.iter().any(|g| g.axis == 0 && g.at == 250.0));
        assert_eq!(r.rect[0], 70.0 + 140.0);
    }

    #[test]
    fn a_resize_grabs_only_its_own_edge() {
        let (inst, frame) = fixture();
        let d = begin_drag(&inst, &frame, 1, Handle::Se, 110.0, 90.0);
        let r = drag_to(&inst, &frame, &d, 130.0, 110.0, false);
        assert_eq!((r.w, r.h), (60.0, 50.0));
        // The origin corner did not move.
        assert_eq!((r.rect[0], r.rect[1]), (70.0, 60.0));
    }

    #[test]
    fn a_resize_cannot_invert_the_box() {
        let (inst, frame) = fixture();
        let d = begin_drag(&inst, &frame, 1, Handle::Se, 110.0, 90.0);
        let r = drag_to(&inst, &frame, &d, 0.0, 0.0, false);
        assert!(r.w >= 1.0 && r.h >= 1.0);
    }

    #[test]
    fn edits_land_in_the_document() {
        let mut doc: SceneDoc = serde_json::from_value(serde_json::json!({
            "format": "pmg.scene",
            "format_version": {"major": 1, "minor": 0},
            "min_reader": 1,
            "id": "t", "name": "t",
            "canvas": {"width": 400, "height": 300},
            "fields": [], "fonts": [], "assets": [],
            "root": {
                "id": "root", "kind": "box",
                "layout": {}, "transform": {}, "style": {},
                "children": [
                    {"id": "chip", "kind": "box", "layout": {"width": {"Px": 40.0}, "height": {"Px": 30.0}},
                     "transform": {}, "style": {}, "children": []}
                ]
            },
            "timelines": []
        }))
        .expect("fixture doc parses");

        apply_move(&mut doc, "chip", 10.0, -5.0).unwrap();
        apply_resize(&mut doc, "chip", 60.0, 50.0).unwrap();
        let chip = &doc.root.children[0];
        assert_eq!(chip.layout.margin.left, 10.0);
        assert_eq!(chip.layout.margin.top, -5.0);
        assert_eq!(chip.layout.width, Dimension::Px(60.0));
        assert_eq!(chip.layout.height, Dimension::Px(50.0));

        assert!(apply_move(&mut doc, "nope", 1.0, 1.0).is_err());
    }

    #[test]
    fn style_edits_land_and_facts_read_them_back() {
        let mut doc: SceneDoc = serde_json::from_value(serde_json::json!({
            "format": "pmg.scene",
            "format_version": {"major": 1, "minor": 0},
            "min_reader": 1,
            "id": "t", "name": "t",
            "canvas": {"width": 400, "height": 300},
            "fields": [], "fonts": [], "assets": [],
            "root": {
                "id": "root", "kind": "box",
                "layout": {}, "transform": {}, "style": {},
                "children": [
                    {"id": "chip", "kind": "box",
                     "layout": {"width": {"Px": 40.0}, "height": {"Px": 30.0}},
                     "transform": {},
                     "style": {"fill": {"rgba": [1.0, 0.0, 0.0, 1.0]}, "radius": 4.0},
                     "children": []}
                ]
            },
            "timelines": []
        }))
        .expect("fixture doc parses");

        let before = node_facts(&doc, "chip").unwrap();
        assert_eq!(before.fill, Some([1.0, 0.0, 0.0, 1.0]));
        assert_eq!(before.radius, 4.0);
        assert_eq!(before.opacity, 1.0);
        assert!(!before.absolute);

        apply_fill(&mut doc, "chip", Some([0.0, 1.0, 0.0, 0.5])).unwrap();
        apply_radius(&mut doc, "chip", -3.0).unwrap(); // clamped, not an error
        apply_opacity(&mut doc, "chip", 1.7).unwrap(); // clamped, not an error
        apply_position(&mut doc, "chip", true).unwrap();

        let after = node_facts(&doc, "chip").unwrap();
        assert_eq!(after.fill, Some([0.0, 1.0, 0.0, 0.5]));
        assert_eq!(after.radius, 0.0);
        assert_eq!(after.opacity, 1.0);
        assert!(after.absolute);

        // Clearing the fill makes the box structural — the round trip must
        // say None, not black.
        apply_fill(&mut doc, "chip", None).unwrap();
        assert_eq!(node_facts(&doc, "chip").unwrap().fill, None);

        assert!(node_facts(&doc, "ghost").is_err());
    }

    #[test]
    fn add_and_delete_reshape_the_tree_and_the_root_survives() {
        let mut doc: SceneDoc = serde_json::from_value(serde_json::json!({
            "format": "pmg.scene",
            "format_version": {"major": 1, "minor": 0},
            "min_reader": 1,
            "id": "t", "name": "t",
            "canvas": {"width": 400, "height": 300},
            "fields": [], "fonts": [], "assets": [],
            "root": {
                "id": "root", "kind": "box",
                "layout": {}, "transform": {}, "style": {},
                "children": [
                    // An existing node already holding the first generated
                    // name, so the counter has to walk past it.
                    {"id": "box-1", "kind": "box", "layout": {}, "transform": {},
                     "style": {}, "children": []}
                ]
            },
            "timelines": []
        }))
        .expect("fixture doc parses");

        // Fresh ids never collide with what is already there.
        let a = apply_add_box(&mut doc, None).unwrap();
        assert_eq!(a, "box-2");
        let b = apply_add_box(&mut doc, Some(&a)).unwrap();
        assert_eq!(b, "box-3");
        // b landed INSIDE a, not beside it.
        assert_eq!(doc.root.children.len(), 2);
        let a_node = doc.root.children.iter().find(|c| c.id == a).unwrap();
        assert_eq!(a_node.children.len(), 1);

        // A new box arrives visible and grabbable: filled, fixed size.
        assert!(a_node.style.fill.is_some());
        assert_eq!(a_node.layout.width, Dimension::Px(240.0));

        // Deleting a container takes its contents with it.
        apply_delete(&mut doc, &a).unwrap();
        assert_eq!(doc.root.children.len(), 1);
        assert!(node_facts(&doc, &b).is_err());

        // The root is the canvas; deleting it is refused with a sentence.
        let err = apply_delete(&mut doc, "root").unwrap_err();
        assert!(err.contains("root"));
        assert!(apply_delete(&mut doc, "ghost").is_err());
    }

    #[test]
    fn timeline_edits_keep_keys_sorted_and_honest() {
        let mut doc: SceneDoc = serde_json::from_value(serde_json::json!({
            "format": "pmg.scene",
            "format_version": {"major": 1, "minor": 0},
            "min_reader": 1,
            "id": "t", "name": "t",
            "canvas": {"width": 400, "height": 300},
            "fields": [], "fonts": [], "assets": [],
            "root": {
                "id": "root", "kind": "box", "layout": {}, "transform": {}, "style": {},
                "children": [{"id": "band", "kind": "box",
                    "layout": {"width": {"Px": 100.0}, "height": {"Px": 40.0}},
                    "transform": {}, "style": {"fill": {"rgba": [1.0,1.0,1.0,1.0]}}, "children": []}]
            },
            "timelines": []
        }))
        .expect("fixture doc parses");

        apply_add_timeline(&mut doc, "In", 700.0).unwrap();
        assert!(
            apply_add_timeline(&mut doc, "In", 500.0).is_err(),
            "names are unique"
        );
        assert!(
            apply_add_timeline(&mut doc, "  ", 500.0).is_err(),
            "a name is required"
        );

        // Keys inserted out of order come out sorted, because eval requires it.
        let ease = pmg_anim::Ease::Cubic([0.16, 1.0, 0.3, 1.0]);
        apply_set_key(
            &mut doc,
            "In",
            "band",
            "transform.translate.x",
            700.0,
            0.0,
            ease,
        )
        .unwrap();
        apply_set_key(
            &mut doc,
            "In",
            "band",
            "transform.translate.x",
            0.0,
            -900.0,
            ease,
        )
        .unwrap();
        let tl = doc.timelines.iter().find(|t| t.name == "In").unwrap();
        let track = &tl.tracks[0];
        assert_eq!(track.target, "band/transform.translate.x");
        assert_eq!(
            track.keys.iter().map(|k| k.t).collect::<Vec<_>>(),
            vec![0.0, 700.0]
        );

        // Setting a key AT an existing time replaces it — editing a value must
        // not stack a twin the evaluator then steps through.
        apply_set_key(
            &mut doc,
            "In",
            "band",
            "transform.translate.x",
            0.0,
            -500.0,
            ease,
        )
        .unwrap();
        let tl = doc.timelines.iter().find(|t| t.name == "In").unwrap();
        assert_eq!(tl.tracks[0].keys.len(), 2);
        assert_eq!(tl.tracks[0].keys[0].v, -500.0);

        // The whitelist: layout cannot be animated, and the sentence says why.
        let err =
            apply_set_key(&mut doc, "In", "band", "layout.width", 0.0, 1.0, ease).unwrap_err();
        assert!(err.contains("layout"));
        assert!(apply_set_key(&mut doc, "In", "ghost", "style.opacity", 0.0, 1.0, ease).is_err());

        // Deleting the last key removes the track; deleting a missing key says so.
        apply_delete_key(&mut doc, "In", "band", "transform.translate.x", 0.0).unwrap();
        apply_delete_key(&mut doc, "In", "band", "transform.translate.x", 700.0).unwrap();
        let tl = doc.timelines.iter().find(|t| t.name == "In").unwrap();
        assert!(tl.tracks.is_empty());
        assert!(apply_delete_key(&mut doc, "In", "band", "transform.translate.x", 0.0).is_err());

        // Retiming MOVES: one key before, one key after, value and ease
        // carried. (The duplicated-key version of this was a live bug — the
        // panel composed delete+set as two session edits over one base.)
        apply_set_key(
            &mut doc,
            "In",
            "band",
            "transform.translate.x",
            100.0,
            -250.0,
            ease,
        )
        .unwrap();
        apply_retime_key(
            &mut doc,
            "In",
            "band",
            "transform.translate.x",
            100.0,
            400.0,
        )
        .unwrap();
        let tl = doc.timelines.iter().find(|t| t.name == "In").unwrap();
        assert_eq!(tl.tracks[0].keys.len(), 1);
        assert_eq!(tl.tracks[0].keys[0].t, 400.0);
        assert_eq!(tl.tracks[0].keys[0].v, -250.0);
        assert!(
            apply_retime_key(&mut doc, "In", "band", "transform.translate.x", 9.0, 10.0).is_err()
        );
        apply_delete_key(&mut doc, "In", "band", "transform.translate.x", 400.0).unwrap();

        apply_set_timeline_duration(&mut doc, "In", 1200.0).unwrap();
        assert_eq!(doc.timelines[0].duration_ms, 1200.0);
        apply_delete_timeline(&mut doc, "In").unwrap();
        assert!(apply_delete_timeline(&mut doc, "In").is_err());
    }

    #[test]
    fn reparent_is_reorder_unparent_and_refusal() {
        // root ── a ── inner, and b beside a.
        let mut doc: SceneDoc = serde_json::from_value(serde_json::json!({
            "format": "pmg.scene",
            "format_version": {"major": 1, "minor": 0},
            "min_reader": 1,
            "id": "t", "name": "t",
            "canvas": {"width": 400, "height": 300},
            "fields": [], "fonts": [], "assets": [],
            "root": {
                "id": "root", "kind": "box", "layout": {}, "transform": {}, "style": {},
                "children": [
                    {"id": "a", "kind": "box", "layout": {}, "transform": {}, "style": {},
                     "children": [
                        {"id": "inner", "kind": "box", "layout": {}, "transform": {},
                         "style": {}, "children": []}
                     ]},
                    {"id": "b", "kind": "box", "layout": {}, "transform": {}, "style": {},
                     "children": []}
                ]
            },
            "timelines": []
        }))
        .expect("fixture doc parses");

        let order = |doc: &SceneDoc| {
            doc.root
                .children
                .iter()
                .map(|c| c.id.clone())
                .collect::<Vec<_>>()
        };

        // Reorder among siblings: b before a. The index counts positions after
        // the node has left its old place.
        apply_reparent(&mut doc, "b", "root", 0).unwrap();
        assert_eq!(order(&doc), vec!["b", "a"]);

        // Un-parent: inner climbs out of a to the root, between b and a.
        apply_reparent(&mut doc, "inner", "root", 1).unwrap();
        assert_eq!(order(&doc), vec!["b", "inner", "a"]);
        assert!(doc.root.children[2].children.is_empty());

        // Reparent: b dives into a. An oversized index clamps to the end.
        apply_reparent(&mut doc, "b", "a", 99).unwrap();
        assert_eq!(order(&doc), vec!["inner", "a"]);
        assert_eq!(doc.root.children[1].children[0].id, "b");

        // Refusals, each with a sentence: the root cannot move, a node cannot
        // enter itself or its own contents, and both ends must exist.
        assert!(apply_reparent(&mut doc, "root", "a", 0).is_err());
        assert!(apply_reparent(&mut doc, "a", "a", 0).is_err());
        assert!(
            apply_reparent(&mut doc, "a", "b", 0).is_err(),
            "b is inside a"
        );
        assert!(apply_reparent(&mut doc, "ghost", "root", 0).is_err());
        assert!(apply_reparent(&mut doc, "b", "ghost", 0).is_err());

        // Naming is presentation: set, trim to nothing, and clear.
        apply_set_name(&mut doc, "a", Some("Backdrop")).unwrap();
        assert_eq!(
            doc.root.children[1].display_name.as_deref(),
            Some("Backdrop")
        );
        apply_set_name(&mut doc, "a", Some("   ")).unwrap();
        assert_eq!(doc.root.children[1].display_name, None);
        assert!(apply_set_name(&mut doc, "ghost", Some("x")).is_err());
    }

    #[test]
    fn text_editors_declare_what_they_reference() {
        use pmg_scene::doc::{Align, Content};

        let mut doc: SceneDoc = serde_json::from_value(serde_json::json!({
            "format": "pmg.scene",
            "format_version": {"major": 1, "minor": 0},
            "min_reader": 1,
            "id": "t", "name": "t",
            "canvas": {"width": 400, "height": 300},
            "fields": [], "fonts": [], "assets": [],
            "root": {
                "id": "root", "kind": "box", "layout": {}, "transform": {}, "style": {},
                "children": []
            },
            "timelines": []
        }))
        .expect("fixture doc parses");

        // A fontless document gets Default declared, so arming can say what
        // is missing instead of rendering nothing.
        let id = apply_add_text(&mut doc, None).unwrap();
        assert_eq!(id, "text-1");
        assert_eq!(doc.fonts.len(), 1);
        assert_eq!(doc.fonts[0].family, "Default");
        let facts = node_facts(&doc, &id).unwrap();
        let text = facts.text.expect("a text node has text facts");
        assert_eq!(text.expr, "Text");
        assert_eq!(text.family, "Default");
        assert_eq!(text.size, 48.0);

        // Typing ${speaker} declares the speaker field — compile refuses
        // undeclared references, so the editor that introduces one carries
        // the declaration.
        apply_set_text_expr(&mut doc, &id, "${speaker} live").unwrap();
        assert_eq!(doc.fields.len(), 1);
        assert_eq!(doc.fields[0].key, "speaker");
        assert_eq!(doc.fields[0].sample.as_deref(), Some("speaker"));
        // Referencing it again does not declare a twin.
        apply_set_text_expr(&mut doc, &id, "${speaker}").unwrap();
        assert_eq!(doc.fields.len(), 1);

        // Size clamps; a new family is declared beside the first.
        apply_set_font_size(&mut doc, &id, 0.0).unwrap();
        apply_set_font_family(&mut doc, &id, "IBM Plex Sans", 600, Some("blake3:ffff")).unwrap();
        // The picked file's hash rides on the declaration — staging needs it.
        assert_eq!(
            doc.fonts
                .iter()
                .find(|f| f.family == "IBM Plex Sans")
                .and_then(|f| f.asset.as_deref()),
            Some("blake3:ffff")
        );
        apply_set_text_align(&mut doc, &id, "center").unwrap();
        let node = find_node(&doc.root, &id).unwrap();
        let Some(Content::Text { font, align, .. }) = &node.content else {
            panic!("still text")
        };
        assert_eq!(font.size, 1.0);
        assert_eq!(font.family, "IBM Plex Sans");
        assert_eq!(font.weight, 600);
        assert_eq!(*align, Align::Center);
        assert_eq!(doc.fonts.len(), 2);

        // The whole compile round-trip holds: declared fields, declared
        // fonts, a valid document.
        assert!(pmg_scene::validate::compile(&doc).is_ok());

        // Editors refuse a non-text node with a sentence.
        let box_id = apply_add_box(&mut doc, None).unwrap();
        assert!(apply_set_text_expr(&mut doc, &box_id, "x")
            .unwrap_err()
            .contains("not a text node"));
        assert!(apply_set_text_align(&mut doc, &id, "diagonal").is_err());
    }

    #[test]
    fn image_editors_declare_assets_once_and_refuse_nonsense() {
        use pmg_scene::doc::{Content, Fit, NodeKind};

        let mut doc: SceneDoc = serde_json::from_value(serde_json::json!({
            "format": "pmg.scene",
            "format_version": {"major": 1, "minor": 0},
            "min_reader": 1,
            "id": "t", "name": "t",
            "canvas": {"width": 400, "height": 300},
            "fields": [], "fonts": [], "assets": [],
            "root": {
                "id": "root", "kind": "box", "layout": {}, "transform": {}, "style": {},
                "children": []
            },
            "timelines": []
        }))
        .expect("fixture doc parses");

        // An image node arrives fitted, and its asset is declared.
        let hash = "blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let id = apply_add_image(&mut doc, None, hash).unwrap();
        assert_eq!(id, "image-1");
        assert_eq!(doc.assets.len(), 1);
        assert_eq!(doc.assets[0].kind, "image");
        let facts = node_facts(&doc, &id).unwrap();
        let img = facts.image.expect("an image node has image facts");
        assert_eq!(img.hash.as_deref(), Some(hash));
        assert_eq!(img.fit, "cover");

        // The same picture on a box reuses the declaration — one hash, one key.
        let box_id = apply_add_box(&mut doc, None).unwrap();
        apply_set_image(&mut doc, &box_id, hash).unwrap();
        assert_eq!(doc.assets.len(), 1);
        // The box keeps its kind and its fill; the image is content.
        let b = find_node(&doc.root, &box_id).unwrap();
        assert_eq!(b.kind, NodeKind::Box);
        assert!(b.style.fill.is_some());

        // Fit modes land where the content is, stills or clips alike.
        apply_set_fit(&mut doc, &box_id, "contain").unwrap();
        let b = find_node(&doc.root, &box_id).unwrap();
        assert!(matches!(
            b.content,
            Some(Content::Image {
                fit: Fit::Contain,
                ..
            })
        ));
        assert!(apply_set_fit(&mut doc, &box_id, "diagonal").is_err());

        // Clearing returns a box to its fill; an image node refuses — it IS
        // its picture — and text keeps its words.
        apply_clear_image(&mut doc, &box_id).unwrap();
        assert!(find_node(&doc.root, &box_id).unwrap().content.is_none());
        assert!(apply_clear_image(&mut doc, &id)
            .unwrap_err()
            .contains("image node"));
        let text_id = apply_add_text(&mut doc, None).unwrap();
        assert!(apply_set_image(&mut doc, &text_id, hash)
            .unwrap_err()
            .contains("one thing"));

        // The compile round-trip holds with the declared asset.
        assert!(pmg_scene::validate::compile(&doc).is_ok());
    }

    #[test]
    fn clip_sizing_and_fields_edit_honestly() {
        let mut doc: SceneDoc = serde_json::from_value(serde_json::json!({
            "format": "pmg.scene",
            "format_version": {"major": 1, "minor": 0},
            "min_reader": 1,
            "id": "t", "name": "t",
            "canvas": {"width": 400, "height": 300},
            "fields": [], "fonts": [], "assets": [],
            "root": {
                "id": "root", "kind": "box", "layout": {}, "transform": {}, "style": {},
                "children": [
                    {"id": "band", "kind": "box",
                     "layout": {"width": {"Px": 200.0}, "height": {"Px": 60.0}},
                     "transform": {}, "style": {}, "children": []}
                ]
            },
            "timelines": []
        }))
        .expect("fixture doc parses");

        // Clip round-trips through the facts.
        apply_set_clip(&mut doc, "band", true).unwrap();
        assert!(node_facts(&doc, "band").unwrap().clip);
        assert!(apply_set_clip(&mut doc, "ghost", true).is_err());

        // Per-axis sizing: width hugs content, height stays fixed.
        apply_set_dimension(&mut doc, "band", "width", None).unwrap();
        let facts = node_facts(&doc, "band").unwrap();
        assert_eq!(facts.width_mode, "content");
        assert_eq!(facts.height_mode, "px");
        apply_set_dimension(&mut doc, "band", "width", Some(0.0)).unwrap(); // clamps
        assert_eq!(node_facts(&doc, "band").unwrap().width_mode, "px");
        assert!(apply_set_dimension(&mut doc, "band", "depth", None).is_err());

        // Fields: declared once, edited by presentation, deleted only when
        // nothing references them — and the refusal names the node.
        apply_add_field(&mut doc, "speaker").unwrap();
        assert!(
            apply_add_field(&mut doc, "speaker").is_err(),
            "keys are unique"
        );
        assert!(apply_add_field(&mut doc, "  ").is_err());
        apply_update_field(
            &mut doc,
            "speaker",
            "Speaker name",
            Some("Pastor Dan"),
            true,
        )
        .unwrap();
        let f = &doc.fields[0];
        assert_eq!(f.label, "Speaker name");
        assert_eq!(f.sample.as_deref(), Some("Pastor Dan"));
        assert!(f.required);

        let text_id = apply_add_text(&mut doc, None).unwrap();
        apply_set_text_expr(&mut doc, &text_id, "${speaker}").unwrap();
        let err = apply_delete_field(&mut doc, "speaker").unwrap_err();
        assert!(err.contains(&text_id), "the refusal names the user");
        apply_set_text_expr(&mut doc, &text_id, "static").unwrap();
        apply_delete_field(&mut doc, "speaker").unwrap();
        assert!(doc.fields.is_empty());
        assert!(apply_delete_field(&mut doc, "speaker").is_err());
    }

    #[test]
    fn video_fields_declare_resolve_and_loop() {
        let mut doc: SceneDoc = serde_json::from_value(serde_json::json!({
            "format": "pmg.scene",
            "format_version": {"major": 1, "minor": 0},
            "min_reader": 1,
            "id": "t", "name": "t",
            "canvas": {"width": 400, "height": 300},
            "fields": [], "fonts": [], "assets": [],
            "root": {
                "id": "root", "kind": "box", "layout": {}, "transform": {}, "style": {},
                "children": []
            },
            "timelines": []
        }))
        .expect("fixture doc parses");

        // A video node arrives full-canvas, field-driven, with its field
        // declared — one template, every clip.
        let id = apply_add_video(&mut doc, None).unwrap();
        assert_eq!(id, "video-1");
        assert_eq!(doc.fields.len(), 1);
        assert_eq!(doc.fields[0].key, "clip");
        let facts = node_facts(&doc, &id).unwrap();
        let v = facts.video.expect("a video node has video facts");
        assert_eq!(v.field.as_deref(), Some("clip"));
        assert_eq!(v.fit, "cover");
        assert!(!v.looping);

        apply_set_loop(&mut doc, &id, true).unwrap();
        assert!(node_facts(&doc, &id).unwrap().video.unwrap().looping);
        assert!(apply_set_loop(&mut doc, "root", true).is_err());

        // The field is protected while the node references it, by name.
        let err = apply_delete_field(&mut doc, "clip").unwrap_err();
        assert!(err.contains(&id));

        // Resolution: the Item's data (or the sample) becomes the asset key.
        let hash = "blake3:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
        let mut data = pmg_scene::bind::Data::new();
        data.insert("clip".into(), hash.into());
        let mut armed = doc.clone();
        pmg_scene::bind::resolve_media_fields(&mut armed, &data);
        let Some(pmg_scene::doc::Content::Video { asset_key, .. }) =
            &find_node(&armed.root, &id).unwrap().content
        else {
            panic!("still a video")
        };
        assert_eq!(asset_key, hash);

        // No data and no sample: the key resolves to nothing, which renders
        // as DEGRADED — the honest answer for an Item that has not chosen.
        let mut bare = doc.clone();
        pmg_scene::bind::resolve_media_fields(&mut bare, &Default::default());
        let Some(pmg_scene::doc::Content::Video { asset_key, .. }) =
            &find_node(&bare.root, &id).unwrap().content
        else {
            panic!()
        };
        assert!(asset_key.is_empty());

        // And the whole document still compiles with the video field declared.
        assert!(pmg_scene::validate::compile(&doc).is_ok());
    }
}
