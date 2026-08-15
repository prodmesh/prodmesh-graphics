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

/// Draw one frame into a texture and read it back as raw RGBA8.
pub struct Canvas {
    device: wgpu::Device,
    queue: wgpu::Queue,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
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
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("pmg-design"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            // Rgba8Unorm, never Rgba8UnormSrgb: no automatic colour conversion
            // in the pipeline, on either target. Rule 3 of the parity rules.
            format: pmg_render::TARGET_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("pmg-design-readback"),
            size: (bytes_per_row * height) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        Ok(Self {
            device,
            queue,
            texture,
            view,
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
        let mut enc = self.device.create_command_encoder(&Default::default());
        {
            let _ = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("pmg-design-clear"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
        }
        self.queue.submit(Some(enc.finish()));

        let nodes = pmg_render::scene::apply_tracks(inst, values);
        renderer.render(&self.device, &self.queue, inst, &nodes, &self.view);
    }

    pub fn begin_readback(&self) {
        let mut enc = self.device.create_command_encoder(&Default::default());
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
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
