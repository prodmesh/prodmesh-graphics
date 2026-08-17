//! `SceneInstance` — a document, its data, and its layout, flattened for the GPU.
//!
//! This is the boundary ADR 0001 names: `pmg-render` takes a `SceneInstance`, a
//! `wgpu::Device`, and a target texture, and nothing else.
//!
//! # What "armed" means, and why the split matters
//!
//! Building a `SceneInstance` is the **expensive** half: solve layout, evaluate
//! bindings, shape text, rasterize glyphs, pack an atlas. It happens once, at
//! arm, off air.
//!
//! Rendering a frame is the **cheap** half: evaluate timelines, apply
//! transforms, draw. Nothing per frame touches layout or shaping.
//!
//! That is the same `layout.*` vs `transform.*` split the scene format makes
//! structurally visible (`docs/specs/scene-format.md` §3.2), carried through to
//! where it actually pays: a lower third sized itself once when data bound, and
//! the reveal is a transform over a cached result.

use pmg_layout::{Rect, Solved};
use pmg_scene::doc::{Content, Node, Rgba, SceneDoc, Transform2D};
use std::collections::BTreeMap;

/// A coverage atlas, packed and ready to upload as a single `R8Unorm` texture.
///
/// **Coverage only** — one byte per pixel, never premultiplied colour. The same
/// rule `pmg-text` enforces at rasterization, carried unchanged to the GPU
/// (ADR 0012).
#[derive(Debug, Clone, Default)]
pub struct CoverageAtlas {
    pub width: u32,
    pub height: u32,
    pub bytes: Vec<u8>,
}

impl CoverageAtlas {
    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }
}

/// One glyph, placed in design space and pointing into the atlas.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlacedGlyph {
    /// Top-left in design space, absolute.
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// Texel rectangle in the atlas.
    pub u: u32,
    pub v: u32,
    pub uw: u32,
    pub uh: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RenderContent {
    /// A filled box. Rounded corners are antialiased analytically.
    Box,
    Text(Vec<PlacedGlyph>),
    /// A decoded still. Video is M3d and will reuse this, swapping the texture
    /// per frame rather than rebuilding the instance.
    Image(PlacedImage),
    /// A node kind this reader does not understand.
    ///
    /// Drawn as a magenta placeholder and marked [`SceneInstance::degraded`].
    /// **Rendering nothing and looking fine is the dangerous behaviour**
    /// (`docs/specs/scene-format.md` §3.4).
    Unknown,
}

/// One drawable node, flattened out of the tree in paint order.
#[derive(Debug, Clone, PartialEq)]
pub struct RenderNode {
    pub id: String,
    /// Index of this node's parent in [`SceneInstance::nodes`], if any.
    ///
    /// Transforms COMPOSE down the tree: animating a band must carry its text
    /// with it. Without this the band slides and the text stays, which looks
    /// like a clipping bug and is actually a missing matrix multiply.
    pub parent: Option<usize>,
    /// Resolved by the layout solver, **once**, at arm.
    pub rect: Rect,
    /// The document's authored transform. Timelines modify a *copy* of this per
    /// frame; the base is never mutated, so an animation is always relative to
    /// the design rather than to wherever the last frame left it.
    pub base_transform: Transform2D,
    pub fill: Option<Rgba>,
    pub radius: f32,
    pub opacity: f32,
    pub clip: bool,
    pub content: RenderContent,
}

/// Everything needed to draw one Item, with nothing left to resolve.
#[derive(Debug, Clone, Default)]
pub struct SceneInstance {
    pub canvas_w: u32,
    pub canvas_h: u32,
    /// Paint order, back to front.
    pub nodes: Vec<RenderNode>,
    pub atlas: CoverageAtlas,
    /// Decoded images, uploaded once at arm and sampled per frame.
    pub images: Vec<ImageData>,
    /// Which image slots are actually clips, and what to feed them.
    ///
    /// **Video is an image whose texture changes.** It draws through the same
    /// pipeline, with the same `fit` resolved the same way; the only thing the
    /// renderer does differently is let the caller overwrite the texture between
    /// frames ([`crate::SceneRenderer::update_image`]). That is what the comment
    /// on [`RenderContent::Image`] promised, and it is why playback needed no
    /// new shader, no new pipeline, and no new bind group layout.
    ///
    /// The asset key rides along because the caller has to know *which* clip to
    /// open. It is an opaque string here — this crate never resolves it, which
    /// is what keeps the no-I/O rule intact.
    pub videos: Vec<VideoSlot>,
    /// Set when the document contained something this reader could not draw.
    /// Surfaced to the operator rather than swallowed.
    pub degraded: bool,
}

/// Supplies glyphs for a text node. Implemented by `pmg-text`.
///
/// A trait so `pmg-render` never depends on a particular text stack — spike S2
/// may yet move it, and the renderer must not notice (ADR 0016's reasoning,
/// applied one layer up).
/// Supplies decoded image pixels for `kind: "image"` nodes.
///
/// Mirrors [`GlyphSource`], and for the same reason: the renderer stays free of
/// IO and of any decoder, so it still compiles to WASM unchanged. The node
/// decodes from its CAS; Design decodes from the browser's fetch. Neither one
/// leaks into here.
pub trait ImageSource {
    /// Premultiplied RGBA8 for an asset key, with its dimensions.
    ///
    /// **Premultiplied**, matching everything else in the compositing path —
    /// `pmg-media` does the multiply at decode. Handing back straight alpha
    /// would darken every soft edge, invisibly until it is over live video.
    fn image(&mut self, asset_key: &str) -> Option<(u32, u32, std::sync::Arc<Vec<u8>>)>;
}

/// A clip bound to one image slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoSlot {
    /// Index into [`SceneInstance::images`] — the texture to overwrite.
    pub image: usize,
    /// The document node, so a player can be reported per node rather than
    /// per anonymous slot.
    pub node_id: String,
    pub asset_key: String,
    pub looping: bool,
}

/// An image, sized and cropped for the box laid out for it.
#[derive(Debug, Clone, PartialEq)]
pub struct PlacedImage {
    /// Index into [`SceneInstance::images`].
    pub image: usize,
    /// Sub-rectangle of the source to draw, normalised. `fit` is resolved here,
    /// at build time, so a frame only ever samples a texture.
    pub uv: [f32; 4],
}

/// A decoded image held by a scene instance.
#[derive(Clone)]
pub struct ImageData {
    pub width: u32,
    pub height: u32,
    /// Shared, so cloning a SceneInstance does not copy a 4K still.
    pub rgba: std::sync::Arc<Vec<u8>>,
}

impl std::fmt::Debug for ImageData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print the pixels: a debug line for a 4K still is 33 MB of hex.
        f.debug_struct("ImageData")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("bytes", &self.rgba.len())
            .finish()
    }
}

/// Resolve `fit` into the sub-rectangle of the source to sample.
///
/// Done once, at build, rather than per frame — and in normalised coordinates
/// so the shader does one texture lookup and no arithmetic about aspect ratios.
pub fn fit_uv(
    fit: pmg_scene::doc::Fit,
    img_w: u32,
    img_h: u32,
    box_w: f32,
    box_h: f32,
) -> [f32; 4] {
    use pmg_scene::doc::Fit;
    if img_w == 0 || img_h == 0 || box_w <= 0.0 || box_h <= 0.0 {
        return [0.0, 0.0, 1.0, 1.0];
    }
    let img_aspect = img_w as f32 / img_h as f32;
    let box_aspect = box_w / box_h;
    match fit {
        // Stretch samples the whole source and lets the quad distort it.
        Fit::Stretch => [0.0, 0.0, 1.0, 1.0],
        // Cover crops the overflowing axis, centred.
        Fit::Cover => {
            if img_aspect > box_aspect {
                let w = box_aspect / img_aspect;
                [(1.0 - w) * 0.5, 0.0, w, 1.0]
            } else {
                let h = img_aspect / box_aspect;
                [0.0, (1.0 - h) * 0.5, 1.0, h]
            }
        }
        // Contain samples the whole source; the LETTERBOX is produced by the
        // quad being smaller than the box, which the caller handles. Sampling
        // outside 0..1 to fake it would smear the edge pixels instead.
        Fit::Contain => [0.0, 0.0, 1.0, 1.0],
    }
}

pub trait GlyphSource {
    /// Shape and rasterize a run, returning glyphs placed relative to the run
    /// origin (baseline-left of the FIRST line), plus the ascent used to
    /// position that baseline.
    ///
    /// `max_width` is the box the solver already decided on, so the wrap the
    /// renderer draws is the wrap the solver measured. Passing anything else
    /// here — the natural width, or nothing — is how a box ends up sized for a
    /// different number of lines than it shows.
    fn run(
        &mut self,
        text: &str,
        font: &pmg_scene::doc::FontSpec,
        max_width: Option<f32>,
        max_lines: usize,
        align: pmg_scene::doc::Align,
    ) -> Option<(Vec<RunGlyph>, f32)>;
}

/// A glyph from [`GlyphSource`], before atlas packing.
#[derive(Debug, Clone)]
pub struct RunGlyph {
    /// Offset from the run origin, in pixels. Fractional — that is the point.
    pub x: f32,
    /// Which line this glyph is on, as a baseline offset in pixels.
    ///
    /// Whole pixels, unlike `x`. Text slides horizontally and never vertically,
    /// so the sub-pixel treatment that stops sliding text shuddering (R3) buys
    /// nothing on this axis and a fractional baseline would just soften every
    /// line differently.
    pub y: f32,
    /// Placement of the coverage bitmap relative to the pen, from rasterization.
    pub left: i32,
    pub top: i32,
    pub width: u32,
    pub height: u32,
    /// Alpha coverage, `width * height` bytes.
    pub coverage: Vec<u8>,
}

impl SceneInstance {
    /// Build from a document, its solved layout, and a glyph source.
    ///
    /// The expensive half. Called at arm, never per frame.
    /// Build with no image source. Convenience for text-and-box documents.
    pub fn build(doc: &SceneDoc, solved: &Solved, glyphs: &mut dyn GlyphSource) -> Self {
        struct NoImages;
        impl ImageSource for NoImages {
            fn image(&mut self, _k: &str) -> Option<(u32, u32, std::sync::Arc<Vec<u8>>)> {
                None
            }
        }
        Self::build_with(doc, solved, glyphs, &mut NoImages)
    }

    pub fn build_with(
        doc: &SceneDoc,
        solved: &Solved,
        glyphs: &mut dyn GlyphSource,
        images: &mut dyn ImageSource,
    ) -> Self {
        let mut inst = SceneInstance {
            canvas_w: doc.canvas.width,
            canvas_h: doc.canvas.height,
            ..Default::default()
        };

        // Collect glyph bitmaps first so the atlas can be packed in one pass —
        // packing incrementally would mean resizing the texture mid-build.
        let mut pending: Vec<(usize, Vec<RunGlyph>, f32)> = Vec::new();

        flatten(
            &doc.root,
            None,
            solved,
            glyphs,
            images,
            &mut inst,
            &mut pending,
        );
        pack_atlas(&mut inst, pending);
        inst
    }
}

#[allow(clippy::too_many_arguments)]
fn flatten(
    node: &Node,
    parent: Option<usize>,
    solved: &Solved,
    glyphs: &mut dyn GlyphSource,
    images: &mut dyn ImageSource,
    inst: &mut SceneInstance,
    pending: &mut Vec<(usize, Vec<RunGlyph>, f32)>,
) {
    // A node the solver omitted was removed by `visible_if`. It is not drawn,
    // and neither are its children — it never existed for this Item.
    let Some(rect) = solved.rect(&node.id) else {
        return;
    };

    let content = match (&node.kind.is_renderable(), &node.content) {
        (false, _) => {
            inst.degraded = true;
            RenderContent::Unknown
        }
        (true, Some(Content::Text { font, .. })) => {
            // The solver already evaluated the binding; re-evaluating here would
            // invite the two answers to differ.
            let text = solved.text.get(&node.id).cloned().unwrap_or_default();
            // The solved box, so the renderer wraps exactly where the solver
            // measured it would.
            let (cap, align) = match &node.content {
                Some(Content::Text {
                    max_lines, align, ..
                }) => (max_lines.map_or(usize::MAX, |n| n.max(1) as usize), *align),
                _ => (usize::MAX, pmg_scene::doc::Align::Start),
            };
            match glyphs.run(&text, font, Some(rect.w), cap, align) {
                Some((run, ascent)) if !run.is_empty() => {
                    pending.push((inst.nodes.len(), run, ascent));
                    RenderContent::Text(Vec::new()) // filled in by pack_atlas
                }
                _ => RenderContent::Text(Vec::new()),
            }
        }
        (true, Some(Content::Image { asset_key, fit })) => {
            match images.image(asset_key) {
                Some((w, h, rgba)) => {
                    let idx = inst.images.len();
                    inst.images.push(ImageData {
                        width: w,
                        height: h,
                        rgba,
                    });
                    RenderContent::Image(PlacedImage {
                        image: idx,
                        uv: fit_uv(*fit, w, h, rect.w, rect.h),
                    })
                }
                None => {
                    // The asset is not staged. DEGRADED, not silently blank —
                    // an empty rectangle where a background should be is the
                    // failure that gets noticed on air rather than before it.
                    inst.degraded = true;
                    RenderContent::Unknown
                }
            }
        }
        (
            true,
            Some(Content::Video {
                asset_key,
                fit,
                looping,
            }),
        ) => {
            // The source hands back the clip's FIRST frame, at the clip's size.
            // That is what an armed-but-not-taken clip shows in preview, and it
            // also sizes the texture the player will overwrite — so a missing
            // clip degrades here, at arm, exactly like a missing still.
            match images.image(asset_key) {
                Some((w, h, rgba)) => {
                    let idx = inst.images.len();
                    inst.images.push(ImageData {
                        width: w,
                        height: h,
                        rgba,
                    });
                    inst.videos.push(VideoSlot {
                        image: idx,
                        node_id: node.id.clone(),
                        asset_key: asset_key.clone(),
                        looping: *looping,
                    });
                    RenderContent::Image(PlacedImage {
                        image: idx,
                        uv: fit_uv(*fit, w, h, rect.w, rect.h),
                    })
                }
                None => {
                    inst.degraded = true;
                    RenderContent::Unknown
                }
            }
        }
        _ => RenderContent::Box,
    };

    let me = inst.nodes.len();
    inst.nodes.push(RenderNode {
        id: node.id.clone(),
        parent,
        rect,
        base_transform: node.transform,
        fill: node.style.fill,
        radius: node.style.radius,
        opacity: node.style.opacity,
        clip: node.clip,
        content,
    });

    for c in &node.children {
        flatten(c, Some(me), solved, glyphs, images, inst, pending);
    }
}

/// Shelf-pack every glyph into one texture.
///
/// Deliberately simple: sort by height, fill rows left to right. A real atlas
/// would use a skyline or MaxRects packer, and would need to when M5 brings a
/// full template library — but for a lower third this wastes a few kilobytes and
/// is obviously correct, which is the better trade today.
fn pack_atlas(inst: &mut SceneInstance, pending: Vec<(usize, Vec<RunGlyph>, f32)>) {
    if pending.is_empty() {
        return;
    }

    // Index every glyph so placement can be written back afterwards.
    let mut all: Vec<(usize, usize)> = Vec::new();
    for (pi, (_, run, _)) in pending.iter().enumerate() {
        for (gi, g) in run.iter().enumerate() {
            // Whitespace rasterizes to nothing; packing a zero-area glyph would
            // put a degenerate rect in the atlas.
            if g.width > 0 && g.height > 0 {
                all.push((pi, gi));
            }
        }
    }
    if all.is_empty() {
        return;
    }

    // Tallest first, so shelves stay tightly packed.
    all.sort_by_key(|(pi, gi)| std::cmp::Reverse(pending[*pi].1[*gi].height));

    const PAD: u32 = 1; // keep bilinear sampling from bleeding between glyphs
    let atlas_w: u32 = 1024;
    let mut placements: BTreeMap<(usize, usize), (u32, u32)> = BTreeMap::new();
    let (mut pen_x, mut pen_y, mut shelf_h) = (PAD, PAD, 0u32);

    for (pi, gi) in &all {
        let g = &pending[*pi].1[*gi];
        if pen_x + g.width + PAD > atlas_w {
            pen_x = PAD;
            pen_y += shelf_h + PAD;
            shelf_h = 0;
        }
        placements.insert((*pi, *gi), (pen_x, pen_y));
        pen_x += g.width + PAD;
        shelf_h = shelf_h.max(g.height);
    }
    let atlas_h = (pen_y + shelf_h + PAD).next_power_of_two();

    let mut bytes = vec![0u8; (atlas_w * atlas_h) as usize];
    for ((pi, gi), (x, y)) in &placements {
        let g = &pending[*pi].1[*gi];
        for row in 0..g.height {
            let src = (row * g.width) as usize;
            let dst = ((y + row) * atlas_w + x) as usize;
            bytes[dst..dst + g.width as usize]
                .copy_from_slice(&g.coverage[src..src + g.width as usize]);
        }
    }

    // Write placements back onto the nodes.
    for (pi, (node_idx, run, ascent)) in pending.iter().enumerate() {
        let origin = inst.nodes[*node_idx].rect;
        let placed: Vec<PlacedGlyph> = run
            .iter()
            .enumerate()
            .filter_map(|(gi, g)| {
                let (u, v) = *placements.get(&(pi, gi))?;
                Some(PlacedGlyph {
                    // The pen sits at the run origin; `left`/`top` place the
                    // bitmap relative to it, and the baseline sits `ascent`
                    // below the box top.
                    x: origin.x + g.x + g.left as f32,
                    y: origin.y + ascent + g.y - g.top as f32,
                    w: g.width as f32,
                    h: g.height as f32,
                    u,
                    v,
                    uw: g.width,
                    uh: g.height,
                })
            })
            .collect();
        inst.nodes[*node_idx].content = RenderContent::Text(placed);
    }

    inst.atlas = CoverageAtlas {
        width: atlas_w,
        height: atlas_h,
        bytes,
    };
}

/// Apply a timeline's evaluated values to a copy of the base transforms.
///
/// **Only `transform.*` and `style.opacity` are animatable in v1.** A track
/// targeting `layout.*` is dropped here rather than honoured, because honouring
/// it would mean relayout on the frame path — the thing the whole split exists
/// to prevent. The compiler already warns about such a track at publish
/// (`docs/specs/scene-format.md` §3.2).
pub fn apply_tracks(inst: &SceneInstance, values: &BTreeMap<String, f32>) -> Vec<FrameNode> {
    inst.nodes
        .iter()
        .map(|n| {
            let mut t = n.base_transform;
            let mut opacity = n.opacity;
            let get = |prop: &str| values.get(&format!("{}/{}", n.id, prop)).copied();

            if let Some(v) = get("transform.translate.x") {
                t.translate[0] = v;
            }
            if let Some(v) = get("transform.translate.y") {
                t.translate[1] = v;
            }
            if let Some(v) = get("transform.rotate") {
                t.rotate = v;
            }
            if let Some(v) = get("transform.scale.x") {
                t.scale[0] = v;
            }
            if let Some(v) = get("transform.scale.y") {
                t.scale[1] = v;
            }
            if let Some(v) = get("style.opacity") {
                opacity = v;
            }
            FrameNode {
                transform: t,
                opacity,
                rect: None,
            }
        })
        .collect()
}

/// Per-frame state for one node: the only thing that changes between frames.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameNode {
    pub transform: Transform2D,
    pub opacity: f32,
    /// Where to draw this node, when that is **not** where layout solved it.
    ///
    /// `None` — the overwhelmingly common case — means use the solved rect, and
    /// the arm/take split stands: layout happened once and a frame only applies
    /// transforms.
    ///
    /// `Some` exists for exactly one thing: a box that **grows or shrinks to fit
    /// new text while it is on air**. That is not a transform — scaling a band
    /// stretches its corner radii and its text with it — it is a different
    /// layout. The tween solves both ends at rebind and interpolates between
    /// them (see [`crate::tween`]), so this is still not a relayout per frame.
    pub rect: Option<pmg_layout::Rect>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracks_apply_to_a_copy_not_to_the_base() {
        // An animation must always be relative to the DESIGN, not to wherever
        // the previous frame left things. Otherwise replaying In twice would
        // compound.
        let inst = SceneInstance {
            nodes: vec![RenderNode {
                id: "band".into(),
                parent: None,
                rect: Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 10.0,
                    h: 10.0,
                },
                base_transform: Transform2D::default(),
                fill: None,
                radius: 0.0,
                opacity: 1.0,
                clip: false,
                content: RenderContent::Box,
            }],
            ..Default::default()
        };

        let mut v = BTreeMap::new();
        v.insert("band/transform.translate.x".to_string(), -900.0);
        let f1 = apply_tracks(&inst, &v);
        let f2 = apply_tracks(&inst, &v);

        assert_eq!(f1[0].transform.translate[0], -900.0);
        assert_eq!(f1, f2, "applying twice must give the same answer");
        assert_eq!(
            inst.nodes[0].base_transform.translate[0], 0.0,
            "the base transform was mutated"
        );
    }

    #[test]
    fn a_layout_track_is_ignored_rather_than_honoured() {
        // Honouring it would mean relayout on the frame path.
        let inst = SceneInstance {
            nodes: vec![RenderNode {
                id: "band".into(),
                parent: None,
                rect: Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 10.0,
                    h: 10.0,
                },
                base_transform: Transform2D::default(),
                fill: None,
                radius: 0.0,
                opacity: 1.0,
                clip: false,
                content: RenderContent::Box,
            }],
            ..Default::default()
        };
        let mut v = BTreeMap::new();
        v.insert("band/layout.width".to_string(), 999.0);
        let f = apply_tracks(&inst, &v);
        assert_eq!(f[0].transform, Transform2D::default());
    }
}
