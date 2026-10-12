//! The output loop: one thread per output group, wgpu in, NDI out.
//!
//! # What happens per frame, and what deliberately does not
//!
//! Per frame: evaluate timelines, apply transforms, draw, read back, send.
//! **Not** per frame: layout, binding, shaping, rasterization, atlas packing.
//! That split is the reason a take fits in a frame at all — the 17.55 ms
//! measurement in `docs/INTEGRATION-NOTES.md` is what regenerating per frame
//! actually costs against a 16.68 ms budget.
//!
//! # Pacing
//!
//! NDI is created with `clock_video`, so the SDK paces the send and the loop
//! free-runs at the right rate without a timer of its own. That is genuinely
//! only adequate for NDI — a DeckLink output schedules against its own clock,
//! and M2's `pmg-out-decklink` owns that. Do not generalise this loop into
//! "the" frame loop.

use anyhow::{bail, Context, Result};
use pmg_render::scene::SceneInstance;
use pmg_render::SceneRenderer;
use std::sync::Arc;

/// A wgpu device plus the readback machinery, reused for every frame.
pub struct Gpu {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub adapter_name: String,
    pub backend: String,
    pub max_texture_2d: u32,
}

impl Gpu {
    pub fn open() -> Result<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
            apply_limit_buckets: false,
        }))
        .context("no wgpu adapter")?;
        let info = adapter.get_info();
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("pmg-node"),
            required_features: wgpu::Features::empty(),
            // The SAME downlevel limits the browser asks for, so a template
            // that renders in Design cannot exceed what a node will accept.
            required_limits:
                wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits()),
            memory_hints: Default::default(),
            experimental_features: Default::default(),
            trace: Default::default(),
        }))
        .context("no wgpu device")?;

        Ok(Self {
            max_texture_2d: adapter.limits().max_texture_dimension_2d,
            adapter_name: info.name.clone(),
            backend: format!("{:?}", info.backend),
            device,
            queue,
        })
    }
}

/// A render target sized to one canvas, kept alive across frames.
pub struct Target {
    pub width: u32,
    pub height: u32,
    /// Composites in linear and encodes on the way out (ADR 0012).
    ///
    /// The pairing lives in `pmg-render` rather than here because every caller
    /// of `SceneRenderer` needs it — the CLI and the spikes had their own
    /// copies of the same two textures before it existed.
    frame: pmg_render::draw::LinearFrame,
    readback: wgpu::Buffer,
    bytes_per_row: u32,
}

impl Target {
    pub fn new(gpu: &Gpu, width: u32, height: u32) -> Result<Self> {
        let bytes_per_row = width * 4;
        if !bytes_per_row.is_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) {
            bail!("canvas width {width} needs row padding, which this M1 path does not do yet");
        }
        let frame = pmg_render::draw::LinearFrame::new(&gpu.device, width, height);
        let readback = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("pmg-node-readback"),
            size: (bytes_per_row * height) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        Ok(Self {
            width,
            height,
            frame,
            readback,
            bytes_per_row,
        })
    }

    /// Where layers draw: the LINEAR composite, not the output texture.
    ///
    /// Callers do not choose. Handing out the 8-bit view would silently put
    /// compositing back in gamma space, and the symptom — antialiased edges too
    /// dark against a dark key — looks like a font problem rather than a
    /// pipeline one.
    pub fn view(&self) -> &wgpu::TextureView {
        self.frame.view()
    }

    /// Convert the composited frame to the output transfer function.
    ///
    /// Once per frame, after every layer and before readback.
    pub fn encode(&self, gpu: &Gpu) {
        self.frame.encode(&gpu.device, &gpu.queue);
    }

    /// Clear to fully transparent.
    ///
    /// Transparent black, not opaque black: the output is a key signal, and an
    /// empty layer must key out entirely rather than punch a black hole in the
    /// programme feed.
    pub fn clear(&self, gpu: &Gpu) {
        self.frame.clear(&gpu.device, &gpu.queue);
    }

    /// Copy the target back to CPU memory as premultiplied RGBA.
    pub fn read(&self, gpu: &Gpu) -> Result<Vec<u8>> {
        // Encode HERE rather than expecting every caller to remember. A frame
        // read without encoding is a whole frame of linear light interpreted as
        // sRGB — washed out, and wrong in a way that looks like a colour
        // setting rather than a missing step.
        self.encode(gpu);
        let mut enc = gpu.device.create_command_encoder(&Default::default());
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
        gpu.queue.submit(Some(enc.finish()));

        let slice = self.readback.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        gpu.device.poll(wgpu::PollType::wait_indefinitely()).ok();
        rx.recv()??;
        let bytes = slice.get_mapped_range().context("mapped range")?.to_vec();
        self.readback.unmap();
        Ok(bytes)
    }
}

/// The node's own YCbCr encode for one SDI output: the linear composite into
/// `v210` fill and key, applying the output's colour configuration (ADR 0012).
///
/// Both planes come back in ONE submit and one wait — two round trips per
/// frame would be two chances to miss the budget for the same picture.
pub struct V210Out {
    encoder: pmg_render::ycbcr::YcbcrEncoder,
    target: pmg_render::ycbcr::YcbcrTarget,
    fill_rb: wgpu::Buffer,
    key_rb: wgpu::Buffer,
    /// Readback rows are padded to wgpu's 256-byte copy alignment; a `v210`
    /// row at 1280 wide is 3456 bytes and is not.
    padded: usize,
}

impl V210Out {
    pub fn new(gpu: &Gpu, width: u32, height: u32) -> Self {
        let target = pmg_render::ycbcr::YcbcrTarget::new(&gpu.device, width, height);
        let padded = target
            .row_bytes()
            .div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as usize)
            * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as usize;
        let rb = |label| {
            gpu.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: (padded * height as usize) as u64,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            })
        };
        Self {
            encoder: pmg_render::ycbcr::YcbcrEncoder::new(&gpu.device),
            fill_rb: rb("pmg-v210-fill-rb"),
            key_rb: rb("pmg-v210-key-rb"),
            target,
            padded,
        }
    }

    /// Encode the target's linear composite and read both planes back,
    /// unpadded — exactly the bytes a DeckLink `v210` frame holds.
    pub fn encode_and_read(
        &self,
        gpu: &Gpu,
        from: &Target,
        levels: &pmg_color::v210::Levels,
    ) -> Result<(Vec<u8>, Vec<u8>)> {
        self.encoder
            .encode(&gpu.device, &gpu.queue, from.view(), &self.target, levels);
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        for (tex, buf) in [
            (self.target.fill(), &self.fill_rb),
            (self.target.key(), &self.key_rb),
        ] {
            enc.copy_texture_to_buffer(
                tex.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: buf,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(self.padded as u32),
                        rows_per_image: Some(self.target.height),
                    },
                },
                tex.size(),
            );
        }
        gpu.queue.submit(Some(enc.finish()));

        let (tx, rx) = std::sync::mpsc::channel();
        for buf in [&self.fill_rb, &self.key_rb] {
            let tx = tx.clone();
            buf.slice(..).map_async(wgpu::MapMode::Read, move |r| {
                let _ = tx.send(r);
            });
        }
        gpu.device.poll(wgpu::PollType::wait_indefinitely()).ok();
        rx.recv()??;
        rx.recv()??;

        let row = self.target.row_bytes();
        let unpad = |buf: &wgpu::Buffer| -> Result<Vec<u8>> {
            let mapped = buf.slice(..).get_mapped_range().context("mapped range")?;
            let mut out = Vec::with_capacity(row * self.target.height as usize);
            for y in 0..self.target.height as usize {
                out.extend_from_slice(&mapped[y * self.padded..y * self.padded + row]);
            }
            Ok(out)
        };
        let fill = unpad(&self.fill_rb);
        let key = unpad(&self.key_rb);
        self.fill_rb.unmap();
        self.key_rb.unmap();
        Ok((fill?, key?))
    }
}

/// Draw one occupant's current frame into the target.
///
/// `load` decides whether this draw starts from nothing or composites over the
/// layer below it, which is what makes a layer STACK rather than a sequence of
/// mutually erasing pictures.
pub fn draw(
    gpu: &Gpu,
    renderer: &SceneRenderer,
    inst: &SceneInstance,
    values: &std::collections::BTreeMap<String, f32>,
    target: &Target,
    load: pmg_render::Load,
) {
    let frame_nodes = pmg_render::scene::apply_tracks(inst, values);
    draw_frames(gpu, renderer, inst, &frame_nodes, target, load);
}

/// Draw a frame list that has already been computed.
///
/// The tween path builds its two lists together — the morphed geometry and the
/// dissolving content have to agree on `t` — so it cannot go through `draw`,
/// which computes its own.
pub fn draw_frames(
    gpu: &Gpu,
    renderer: &SceneRenderer,
    inst: &SceneInstance,
    frames: &[pmg_render::scene::FrameNode],
    target: &Target,
    load: pmg_render::Load,
) {
    renderer.render_with(
        &gpu.device,
        &gpu.queue,
        inst,
        frames,
        target.view(),
        (target.width, target.height),
        load,
    );
}

/// A renderer per occupied layer, kept across frames.
///
/// The renderer owns the glyph atlas and the pipelines, so building one is
/// expensive and building one PER LAYER PER FRAME is the 17.55 ms mistake from
/// `docs/INTEGRATION-NOTES.md` wearing a different hat. The first version of the
/// node's frame loop held a single renderer and rebuilt it whenever the layer
/// being drawn changed — which is every layer, every frame, the moment a second
/// layer is occupied.
#[derive(Default)]
pub struct RendererCache {
    /// layer name -> (the instance it was built for, the renderer)
    ///
    /// Keyed on the **identity of the instance**, not on a name for it.
    ///
    /// It used to be keyed on `item_id|template_rev`, which was correct while
    /// the only way an instance could change was re-arming a different Item.
    /// A live rebind (M3g) changes neither: same Item, same template, new data
    /// and therefore a **new glyph atlas**. The stale renderer was handed back
    /// and used to draw the new instance, whose glyph coordinates indexed into
    /// an atlas built for the old text — text rendered as garbage on air while
    /// the box tweened perfectly, because boxes do not touch the atlas.
    ///
    /// The `Arc` is held rather than its address compared, so a freed instance
    /// cannot have its allocation reused by a new one and silently match.
    entries: std::collections::HashMap<String, (Arc<SceneInstance>, SceneRenderer)>,
}

impl RendererCache {
    /// Get the renderer for a layer, rebuilding only if the occupant changed.
    /// Get the renderer for a layer, rebuilding only if the instance changed.
    ///
    /// Takes the `Arc` rather than a reference plus a key, so it is not possible
    /// for a caller to describe an instance inaccurately. That was the shape of
    /// the bug this replaced.
    pub fn get(&mut self, gpu: &Gpu, layer: &str, inst: &Arc<SceneInstance>) -> &SceneRenderer {
        let slot = self.entries.entry(layer.to_string()).or_insert_with(|| {
            (
                inst.clone(),
                SceneRenderer::new(&gpu.device, &gpu.queue, inst),
            )
        });
        if !Arc::ptr_eq(&slot.0, inst) {
            *slot = (
                inst.clone(),
                SceneRenderer::new(&gpu.device, &gpu.queue, inst),
            );
        }
        &slot.1
    }

    /// Drop renderers for layers that are no longer occupied, so a cleared
    /// layer does not hold its atlas for the rest of the service.
    pub fn retain(&mut self, live: &std::collections::HashSet<String>) {
        self.entries.retain(|k, _| live.contains(k));
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}
