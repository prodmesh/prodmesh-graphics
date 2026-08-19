//! The Design-mode surface: compile a template, and render it the way a node
//! will.
//!
//! # This is the whole premise of ADR 0001
//!
//! The browser runs **the same `pmg-scene`, `pmg-layout`, `pmg-text`, and
//! `pmg-render` code the playout node runs**, compiled to WASM. Two
//! consequences, and both are the point:
//!
//! - `template_rev` computed here is byte-identical to the one `pmgctl`
//!   computes, because it is the same canonical CBOR encoder over the same
//!   document. The designer's publish and the node's fetch agree by
//!   construction rather than by convention.
//! - The canvas is not an approximation of the output. It is the output, minus
//!   the colour conversion that happens at the wire boundary.
//!
//! # Fonts arrive as bytes, and that is not a limitation
//!
//! There is no system font database here and there must not be. A template
//! naming a system font would render one way on a designer's laptop and another
//! on a booth Mac, silently. Fonts are assets carried by content hash; the
//! browser fetches them from the CAS exactly as a node does.
//!
//! # Readback
//!
//! [`render_rgba`] returns **raw RGBA8** rather than drawing to a canvas
//! element. The caller decides what to do with it — `putImageData` for display,
//! or a byte comparison for the parity harness. Going through `toDataURL` would
//! put the browser's colour management in the middle, which is fine for a
//! picture and fatal for a measurement.

use pmg_render::scene::SceneInstance;
use pmg_render::text_source::TextEngineGlyphs;
use pmg_render::{timeline, SceneRenderer};
use pmg_scene::doc::SceneDoc;
use pmg_text::{FontDb, Measurer, TextEngine};

/// What a compile produced. Mirrors `pmg_scene::validate::Manifest` plus the
/// bytes, so one call gives the caller everything a publish needs.
pub struct Compiled {
    pub template_rev: String,
    pub scene: Vec<u8>,
    pub manifest_json: String,
}

/// Compile a Template Document (the JSON mirror) to canonical CBOR plus its
/// manifest.
pub fn compile(doc_json: &str) -> Result<Compiled, String> {
    let doc: SceneDoc =
        serde_json::from_str(doc_json).map_err(|e| format!("not a template: {e}"))?;
    let (scene, manifest) = pmg_scene::validate::compile(&doc).map_err(|e| e.to_string())?;
    Ok(Compiled {
        template_rev: manifest.template_rev.clone(),
        scene,
        manifest_json: serde_json::to_string(&manifest).map_err(|e| e.to_string())?,
    })
}

/// Build a font database from raw face bytes.
///
/// Each face is registered under every family the document names, because M1
/// has no font-asset mapping yet. That is provisional and says so, rather than
/// pretending to match families it cannot resolve.
fn font_db(doc: &SceneDoc, faces: &[Vec<u8>]) -> Result<FontDb, String> {
    let mut db = FontDb::new();
    let mut families: Vec<(String, u16)> = doc
        .fonts
        .iter()
        .map(|f| (f.family.clone(), f.weight))
        .collect();
    families.sort();
    families.dedup();

    // A template that names fonts and is given none must FAIL, not render.
    //
    // With an empty database every shaping call returns None, so text simply
    // does not appear — and an empty canvas is indistinguishable from a
    // template with nothing in it. That is the same class of failure S2 found
    // with missing scripts: silently drawing nothing (or tofu) rather than
    // saying what is missing. Name the families, so the answer is actionable.
    if !families.is_empty() && faces.is_empty() {
        let names: Vec<&str> = families.iter().map(|(f, _)| f.as_str()).collect();
        return Err(format!(
            "no font supplied. This template needs {} — add it as an asset so every machine renders it the same way.",
            names.join(", ")
        ));
    }
    if families.is_empty() {
        families.push(("Default".into(), 400));
    }
    for bytes in faces {
        for (family, weight) in &families {
            db.add_face(bytes.clone(), family.clone(), *weight)
                .map_err(|e| format!("font: {e}"))?;
        }
    }
    Ok(db)
}

/// Everything expensive, done once — exactly as a node does it at arm.
pub struct Armed {
    pub instance: SceneInstance,
    pub doc: SceneDoc,
    /// Field keys whose `visible_if` collapsed them out of the layout. The
    /// designer wants to see this: it is why the band got shorter.
    pub omitted: Vec<String>,
    pub degraded: bool,
}

pub fn arm(doc_json: &str, data_json: &str, faces: &[Vec<u8>]) -> Result<Armed, String> {
    let doc: SceneDoc =
        serde_json::from_str(doc_json).map_err(|e| format!("not a template: {e}"))?;
    let supplied: std::collections::BTreeMap<String, String> =
        serde_json::from_str(data_json).map_err(|e| format!("not item data: {e}"))?;

    // Fall back to each field's sample. Samples are what Design previews with
    // and what a golden image renders, so an empty box in the editor still
    // shows the designer what the template is for.
    let mut data: pmg_scene::bind::Data = doc
        .fields
        .iter()
        .filter_map(|f| f.sample.clone().map(|s| (f.key.clone(), s)))
        .collect();
    for (k, v) in supplied {
        data.insert(k, v);
    }

    let measurer = Measurer::new(TextEngine::new(font_db(&doc, faces)?));
    let solved = pmg_layout::solve(&doc, &data, &measurer).map_err(|e| e.to_string())?;
    let mut engine = measurer.into_engine();
    let instance = SceneInstance::build(&doc, &solved, &mut TextEngineGlyphs::new(&mut engine));

    Ok(Armed {
        omitted: solved.omitted.clone(),
        degraded: instance.degraded,
        instance,
        doc,
    })
}

// ---------------------------------------------------------------------------
// Interaction — the ADR 0019 surface, session-shaped.
//
// Everything with a decision in it stays in `pmg_render::interact`; this layer
// only finds nodes by id, serializes answers, and applies a finished drag to a
// COPY of the document. The copy matters: the armed instance keeps rendering
// the old document until the caller re-arms with the returned one, so the
// canvas never shows a half-applied edit.
// ---------------------------------------------------------------------------

use pmg_render::interact;
use pmg_render::scene::FrameNode;

/// The resting frame — base transforms, no timeline. Editing happens at rest:
/// a designer drags the design, not a moment of its In animation.
pub fn rest_frame(armed: &Armed) -> Vec<FrameNode> {
    pmg_render::scene::apply_tracks(&armed.instance, &Default::default())
}

fn index_of(armed: &Armed, id: &str) -> Result<usize, String> {
    armed
        .instance
        .nodes
        .iter()
        .position(|n| n.id == id)
        .ok_or_else(|| format!("no node {id} in this instance"))
}

/// The node id under a point, or None.
pub fn hit(armed: &Armed, frame: &[FrameNode], x: f32, y: f32) -> Option<String> {
    interact::hit_test(&armed.instance, frame, x, y).map(|i| armed.instance.nodes[i].id.clone())
}

/// Selection chrome for a node, as JSON (`interact::Handles`).
pub fn handles_json(armed: &Armed, frame: &[FrameNode], id: &str) -> Result<String, String> {
    let idx = index_of(armed, id)?;
    let h = interact::handles_for(&armed.instance, frame, idx);
    serde_json::to_string(&h).map_err(|e| e.to_string())
}

pub fn begin_drag(
    armed: &Armed,
    frame: &[FrameNode],
    id: &str,
    handle: &str,
    x: f32,
    y: f32,
) -> Result<interact::Drag, String> {
    let idx = index_of(armed, id)?;
    let handle = interact::Handle::parse(handle).ok_or_else(|| format!("no handle {handle}"))?;
    Ok(interact::begin_drag(
        &armed.instance,
        frame,
        idx,
        handle,
        x,
        y,
    ))
}

/// The current drag proposal, as JSON (`interact::DragResult`).
pub fn drag_json(
    armed: &Armed,
    frame: &[FrameNode],
    drag: &interact::Drag,
    x: f32,
    y: f32,
    snap: bool,
) -> Result<String, String> {
    let r = interact::drag_to(&armed.instance, frame, drag, x, y, snap);
    serde_json::to_string(&r).map_err(|e| e.to_string())
}

/// Finish a drag: apply what it proposes to a copy of the document and return
/// that document as JSON — or None when the drag changed nothing, so a click
/// that never moved does not dirty the file.
pub fn end_drag(
    armed: &Armed,
    frame: &[FrameNode],
    drag: &interact::Drag,
    x: f32,
    y: f32,
    snap: bool,
) -> Result<Option<String>, String> {
    let r = interact::drag_to(&armed.instance, frame, drag, x, y, snap);
    let id = armed.instance.nodes[drag.node].id.clone();
    let mut doc = armed.doc.clone();
    if drag.handle == interact::Handle::Move {
        if r.dx == 0.0 && r.dy == 0.0 {
            return Ok(None);
        }
        interact::apply_move(&mut doc, &id, r.dx, r.dy)?;
    } else {
        interact::apply_resize(&mut doc, &id, r.w, r.h)?;
    }
    serde_json::to_string(&doc)
        .map(Some)
        .map_err(|e| e.to_string())
}

/// Move a node by a fixed amount — the keyboard's arrow-nudge path, and the
/// one a test can drive without a pointer.
pub fn nudge(armed: &Armed, id: &str, dx: f32, dy: f32) -> Result<String, String> {
    let mut doc = armed.doc.clone();
    interact::apply_move(&mut doc, id, dx, dy)?;
    serde_json::to_string(&doc).map_err(|e| e.to_string())
}

/// Fix a node's size from the inspector's inputs.
pub fn set_size(armed: &Armed, id: &str, w: f32, h: f32) -> Result<String, String> {
    let mut doc = armed.doc.clone();
    interact::apply_resize(&mut doc, id, w, h)?;
    serde_json::to_string(&doc).map_err(|e| e.to_string())
}

/// The authored facts for one node, as JSON (`interact::NodeFacts`) — what
/// the inspector shows, as distinct from the solved geometry it also shows.
pub fn facts_json(armed: &Armed, id: &str) -> Result<String, String> {
    let f = interact::node_facts(&armed.doc, id)?;
    serde_json::to_string(&f).map_err(|e| e.to_string())
}

fn edited_json(doc: pmg_scene::doc::SceneDoc) -> Result<String, String> {
    serde_json::to_string(&doc).map_err(|e| e.to_string())
}

pub fn set_fill(armed: &Armed, id: &str, fill: Option<[f32; 4]>) -> Result<String, String> {
    let mut doc = armed.doc.clone();
    interact::apply_fill(&mut doc, id, fill)?;
    edited_json(doc)
}

pub fn set_radius(armed: &Armed, id: &str, radius: f32) -> Result<String, String> {
    let mut doc = armed.doc.clone();
    interact::apply_radius(&mut doc, id, radius)?;
    edited_json(doc)
}

pub fn set_opacity(armed: &Armed, id: &str, opacity: f32) -> Result<String, String> {
    let mut doc = armed.doc.clone();
    interact::apply_opacity(&mut doc, id, opacity)?;
    edited_json(doc)
}

pub fn set_position(armed: &Armed, id: &str, absolute: bool) -> Result<String, String> {
    let mut doc = armed.doc.clone();
    interact::apply_position(&mut doc, id, absolute)?;
    edited_json(doc)
}

/// Add a box under `parent` (root when None). Returns the new node's id AND
/// the edited document — the caller selects what it just made.
pub fn add_box(armed: &Armed, parent: Option<&str>) -> Result<(String, String), String> {
    let mut doc = armed.doc.clone();
    let id = interact::apply_add_box(&mut doc, parent)?;
    Ok((id, edited_json(doc)?))
}

/// Delete a node and everything inside it. The root is refused with a
/// sentence.
pub fn delete_node(armed: &Armed, id: &str) -> Result<String, String> {
    let mut doc = armed.doc.clone();
    interact::apply_delete(&mut doc, id)?;
    edited_json(doc)
}

/// Move a node to `(new_parent, index)` — the layer tree's drop, covering
/// reorder, reparent, and un-parent in one operation. Cycles and the root are
/// refused with sentences.
pub fn reparent(armed: &Armed, id: &str, new_parent: &str, index: usize) -> Result<String, String> {
    let mut doc = armed.doc.clone();
    interact::apply_reparent(&mut doc, id, new_parent, index)?;
    edited_json(doc)
}

/// Name a layer for the eye; `None` returns it to showing its id. The id
/// underneath never changes, so timelines and fields keep addressing it.
pub fn set_name(armed: &Armed, id: &str, name: Option<&str>) -> Result<String, String> {
    let mut doc = armed.doc.clone();
    interact::apply_set_name(&mut doc, id, name)?;
    edited_json(doc)
}

// -- Timeline edits ---------------------------------------------------------

pub fn add_timeline(armed: &Armed, name: &str, duration_ms: f32) -> Result<String, String> {
    let mut doc = armed.doc.clone();
    interact::apply_add_timeline(&mut doc, name, duration_ms)?;
    edited_json(doc)
}

pub fn delete_timeline(armed: &Armed, name: &str) -> Result<String, String> {
    let mut doc = armed.doc.clone();
    interact::apply_delete_timeline(&mut doc, name)?;
    edited_json(doc)
}

pub fn set_timeline_duration(armed: &Armed, name: &str, ms: f32) -> Result<String, String> {
    let mut doc = armed.doc.clone();
    interact::apply_set_timeline_duration(&mut doc, name, ms)?;
    edited_json(doc)
}

/// Set a key. `ease_json` is a serde-encoded `pmg_anim::Ease` — "linear",
/// `{"cubic":[x1,y1,x2,y2]}` or `{"steps":n}` — so the wire speaks the same
/// names the document does.
pub fn set_key(
    armed: &Armed,
    timeline: &str,
    node_id: &str,
    prop: &str,
    t: f32,
    v: f32,
    ease_json: &str,
) -> Result<String, String> {
    let ease: pmg_anim::Ease =
        serde_json::from_str(ease_json).map_err(|e| format!("not an easing: {e}"))?;
    let mut doc = armed.doc.clone();
    interact::apply_set_key(&mut doc, timeline, node_id, prop, t, v, ease)?;
    edited_json(doc)
}

pub fn delete_key(
    armed: &Armed,
    timeline: &str,
    node_id: &str,
    prop: &str,
    t: f32,
) -> Result<String, String> {
    let mut doc = armed.doc.clone();
    interact::apply_delete_key(&mut doc, timeline, node_id, prop, t)?;
    edited_json(doc)
}

/// Every track's evaluated value at `t_ms`, as JSON `{ "node/prop": value }`.
/// This is THE evaluation — `pmg_anim` through `timeline::evaluate` — so the
/// editor's "value here" and the renderer's pixels cannot disagree.
pub fn values_at_json(armed: &Armed, timeline: Option<&str>, t_ms: f32) -> Result<String, String> {
    serde_json::to_string(&values_at(armed, timeline, t_ms)).map_err(|e| e.to_string())
}

/// The timeline values at a moment, for the scrubber.
///
/// `None` for the timeline name means the resting state — no timeline applied,
/// which is what a template looks like with nothing playing.
pub fn values_at(
    armed: &Armed,
    timeline_name: Option<&str>,
    t_ms: f32,
) -> std::collections::BTreeMap<String, f32> {
    match timeline_name.and_then(|n| timeline::find(&armed.doc, n)) {
        Some(tl) => timeline::evaluate(tl, t_ms),
        None => Default::default(),
    }
}

/// Draw one frame and read it back as raw RGBA8.
///
/// Composites through [`pmg_render::draw::LinearFrame`] — linear float, then
/// the encode pass — because that is the node's pipeline (ADR 0012) and the
/// scene pipelines target the linear format. Rendering into an 8-bit texture
/// directly stopped being possible when the linear composite landed, and this
/// canvas doing exactly that was why Design drew nothing: every draw failed
/// pipeline/attachment validation, visible only as a console warning.
pub struct Canvas {
    device: wgpu::Device,
    queue: wgpu::Queue,
    frame: pmg_render::draw::LinearFrame,
    readback: wgpu::Buffer,
    width: u32,
    height: u32,
    bytes_per_row: u32,
}

impl Canvas {
    pub fn new(
        device: wgpu::Device,
        queue: wgpu::Queue,
        width: u32,
        height: u32,
    ) -> Result<Self, String> {
        let bytes_per_row = width * 4;
        if !bytes_per_row.is_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) {
            return Err(format!(
                "canvas width {width} needs row padding, which this path does not do"
            ));
        }
        let frame = pmg_render::draw::LinearFrame::new(&device, width, height);
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("pmg-design-readback"),
            size: (bytes_per_row * height) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        Ok(Self {
            device,
            queue,
            frame,
            readback,
            width,
            height,
            bytes_per_row,
        })
    }

    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }
    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    /// Clear to **transparent** black, then draw.
    ///
    /// Transparent, not opaque: this is a key signal, and the designer must see
    /// what will actually key. A checkerboard belongs behind the canvas in CSS,
    /// never baked into the pixels.
    pub fn draw(
        &self,
        renderer: &SceneRenderer,
        inst: &SceneInstance,
        values: &std::collections::BTreeMap<String, f32>,
    ) {
        // Transparent, not opaque: this is a key signal, and the designer must
        // see what will actually key.
        self.frame.clear(&self.device, &self.queue);
        let nodes = pmg_render::scene::apply_tracks(inst, values);
        renderer.render(&self.device, &self.queue, inst, &nodes, self.frame.view());
    }

    pub fn begin_readback(&self) {
        // Linear → encoded, once, at output (ADR 0012). What is read back is
        // what a node would put on the wire.
        self.frame.encode(&self.device, &self.queue);
        let mut enc = self.device.create_command_encoder(&Default::default());
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: self.frame.output(),
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &self.readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(self.bytes_per_row),
                    rows_per_image: Some(self.height),
                },
            },
            wgpu::Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit(Some(enc.finish()));
    }

    pub fn buffer(&self) -> &wgpu::Buffer {
        &self.readback
    }
}

#[cfg(test)]
mod interaction_tests {
    use super::*;

    /// A boxes-only template: no fonts needed, so the whole arm → hit → drag →
    /// re-arm loop runs natively — the same code the browser runs, tested
    /// without a browser (the reason this file exists outside `design_api`).
    fn doc_json() -> String {
        serde_json::json!({
            "format": "pmg.scene",
            "format_version": {"major": 1, "minor": 0},
            "min_reader": 1,
            "id": "t", "name": "t",
            "canvas": {"width": 400, "height": 300},
            "fields": [], "fonts": [], "assets": [],
            "root": {
                "id": "root", "kind": "box",
                "layout": {"width": {"Px": 400.0}, "height": {"Px": 300.0}},
                "transform": {}, "style": {},
                "children": [
                    {"id": "chip", "kind": "box",
                     "layout": {"width": {"Px": 120.0}, "height": {"Px": 40.0}},
                     "transform": {},
                     "style": {"fill": {"rgba": [1.0, 1.0, 1.0, 1.0]}},
                     "children": []}
                ]
            },
            "timelines": []
        })
        .to_string()
    }

    #[test]
    fn the_whole_loop_arm_hit_drag_rearm() {
        let armed = arm(&doc_json(), "{}", &[]).expect("arms without fonts");
        let frame = rest_frame(&armed);

        // The chip lays out at the origin and is hit; empty canvas is not —
        // the root has no fill, and structure is transparent to clicks.
        assert_eq!(hit(&armed, &frame, 60.0, 20.0), Some("chip".into()));
        assert_eq!(hit(&armed, &frame, 300.0, 200.0), None);

        // Drag it 100 right, 60 down (snap off), and finish.
        let drag = begin_drag(&armed, &frame, "chip", "move", 60.0, 20.0).unwrap();
        let doc2 = end_drag(&armed, &frame, &drag, 160.0, 80.0, false)
            .unwrap()
            .expect("a real move edits the document");

        // Re-arm the returned document: the chip now lives where it was
        // dropped, by the same solver that will render it on air.
        let armed2 = arm(&doc2, "{}", &[]).expect("edited doc still arms");
        let frame2 = rest_frame(&armed2);
        assert_eq!(hit(&armed2, &frame2, 60.0, 20.0), None);
        assert_eq!(hit(&armed2, &frame2, 160.0, 80.0), Some("chip".into()));

        // A drag that never moved is a click, not an edit.
        let still = begin_drag(&armed, &frame, "chip", "move", 60.0, 20.0).unwrap();
        assert!(end_drag(&armed, &frame, &still, 60.0, 20.0, false)
            .unwrap()
            .is_none());
    }

    #[test]
    fn resize_fixes_the_size_in_pixels() {
        let armed = arm(&doc_json(), "{}", &[]).expect("arms");
        let frame = rest_frame(&armed);
        let drag = begin_drag(&armed, &frame, "chip", "se", 120.0, 40.0).unwrap();
        let doc2 = end_drag(&armed, &frame, &drag, 180.0, 90.0, false)
            .unwrap()
            .expect("a resize edits the document");
        let armed2 = arm(&doc2, "{}", &[]).expect("arms");
        let chip = armed2
            .instance
            .nodes
            .iter()
            .find(|n| n.id == "chip")
            .unwrap();
        assert_eq!((chip.rect.w, chip.rect.h), (180.0, 90.0));
    }

    #[test]
    fn handles_and_nudge_speak_the_inspector_language() {
        let armed = arm(&doc_json(), "{}", &[]).expect("arms");
        let frame = rest_frame(&armed);
        let h = handles_json(&armed, &frame, "chip").unwrap();
        assert!(h.contains("\"bbox\":[0.0,0.0,120.0,40.0]"));

        let doc2 = nudge(&armed, "chip", 1.0, 0.0).unwrap();
        let armed2 = arm(&doc2, "{}", &[]).expect("arms");
        let h2 = handles_json(&armed2, &rest_frame(&armed2), "chip").unwrap();
        assert!(h2.contains("\"bbox\":[1.0,0.0,120.0,40.0]"));

        assert!(handles_json(&armed, &frame, "ghost").is_err());
    }

    #[test]
    fn reparent_and_rename_round_trip_the_document() {
        let armed = arm(&doc_json(), "{}", &[]).expect("arms");

        // The chip named for the eye, then moved into a fresh sibling — and
        // the edited documents still arm, which is the editor's real contract.
        let (box_id, doc2) = add_box(&armed, None).unwrap();
        let armed2 = arm(&doc2, "{}", &[]).expect("arms");
        let doc3 = set_name(&armed2, "chip", Some("Title chip")).unwrap();
        let armed3 = arm(&doc3, "{}", &[]).expect("arms");
        let doc4 = reparent(&armed3, "chip", &box_id, 0).unwrap();
        let armed4 = arm(&doc4, "{}", &[]).expect("arms");

        let parent = &armed4.doc.root.children[0];
        assert_eq!(parent.id, box_id);
        assert_eq!(parent.children[0].id, "chip");
        assert_eq!(
            parent.children[0].display_name.as_deref(),
            Some("Title chip")
        );

        assert!(reparent(&armed4, &box_id, "chip", 0).is_err(), "no cycles");
    }
}
