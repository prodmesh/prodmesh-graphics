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
use pmg_scene::doc::{Dimension, Node, SceneDoc};
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
        if n.fill.is_none() && matches!(n.content, RenderContent::Box) {
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
}
