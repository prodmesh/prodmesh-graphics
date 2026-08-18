//! The render pass: a [`SceneInstance`] plus per-frame transforms, onto a
//! texture.
//!
//! **No I/O, no platform.** Device, queue, instance, frame state, target view —
//! that is the whole signature. Readback is the caller's problem because how you
//! await it differs by platform (ADR 0004).
//!
//! Nodes are drawn back to front in tree order: painter's algorithm. A depth
//! buffer would be wrong here — everything is translucent, and a lower third is
//! a handful of nodes, not a scene graph worth sorting.

use crate::scene::{FrameNode, RenderContent, SceneInstance};
use crate::Load;
use crate::{COMPOSITE_FORMAT, TARGET_FORMAT};

/// Uniform block. Must match `Node` in `scene.wgsl` exactly.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct NodeUniform {
    rect: [f32; 4],
    color: [f32; 4],
    xform: [f32; 4],
    xform_t: [f32; 2],
    radius: f32,
    opacity: f32,
    canvas: [f32; 2],
    px_per_unit: f32,
    _pad: f32,
    /// Sub-rectangle of an image to sample: x, y, w, h, normalised. `fit` is
    /// resolved at build time so a frame never reasons about aspect ratios.
    uv: [f32; 4],
}

const UNIFORM_SIZE: u64 = 96;
const _: () = assert!(std::mem::size_of::<NodeUniform>() as u64 == UNIFORM_SIZE);

fn uniform_bytes(u: &NodeUniform) -> [u8; UNIFORM_SIZE as usize] {
    let mut buf = [0u8; UNIFORM_SIZE as usize];
    let f = [
        u.rect[0],
        u.rect[1],
        u.rect[2],
        u.rect[3],
        u.color[0],
        u.color[1],
        u.color[2],
        u.color[3],
        u.xform[0],
        u.xform[1],
        u.xform[2],
        u.xform[3],
        u.xform_t[0],
        u.xform_t[1],
        u.radius,
        u.opacity,
        u.canvas[0],
        u.canvas[1],
        u.px_per_unit,
        u._pad,
        u.uv[0],
        u.uv[1],
        u.uv[2],
        u.uv[3],
    ];
    for (i, v) in f.iter().enumerate() {
        buf[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
    }
    buf
}

// The affine build, the world composition, and the clip rects live in
// `crate::interact`, shared with the hit test — so where a node is DRAWN and
// where it is CLICKABLE are one computation, not two that agree until they
// don't (ADR 0019).

/// Compiled pipelines and the uploaded atlas for one instance.
pub struct SceneRenderer {
    pipeline_box: wgpu::RenderPipeline,
    pipeline_text: wgpu::RenderPipeline,
    pipeline_unknown: wgpu::RenderPipeline,
    pipeline_image: wgpu::RenderPipeline,
    /// One bind group per image slot, built once at arm.
    image_binds: Vec<wgpu::BindGroup>,
    /// The textures behind those bind groups, with their sizes.
    ///
    /// A still is written once and never touched again; a clip writes here every
    /// frame. Keeping the handle costs nothing and is the whole of what playback
    /// needed from this crate.
    image_texs: Vec<(wgpu::Texture, u32, u32)>,
    bind_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    atlas_view: wgpu::TextureView,
    atlas_size: (u32, u32),
}

/// Linear light to the output transfer function — the "encode once, at output"
/// half of ADR 0012.
///
/// Separate from [`SceneRenderer`] because a renderer draws ONE layer and a
/// frame is a stack of them. The whole stack composites in linear and is
/// encoded once; encoding per layer would blend the layer above into
/// gamma-encoded values and put the bug back between layers instead of inside
/// them.
pub struct Encoder {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
}

impl Encoder {
    pub fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("pmg-encode"),
            source: wgpu::ShaderSource::Wgsl(include_str!("scene.wgsl").into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("pmg-encode-bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering),
                    count: None,
                },
            ],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("pmg-encode-pl"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("pmg-encode-pipe"),
            layout: Some(&pl),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_encode"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_encode"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: TARGET_FORMAT,
                    // REPLACE, not blend. This writes the finished frame; the
                    // compositing already happened, in linear.
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        // NEAREST on every field. This pass is 1:1 with the target, so there is
        // nothing to filter, and an unstated filter mode is exactly the kind of
        // default that differs between backends (parity rule 4).
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("pmg-encode-sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        Self {
            pipeline,
            layout,
            sampler,
        }
    }

    /// Encode `linear` into `target`, replacing it.
    pub fn encode(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        linear: &wgpu::TextureView,
        target: &wgpu::TextureView,
    ) {
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("pmg-encode-bg"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(linear),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        let mut enc = device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("pmg-encode-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: None,
                    depth_slice: None,
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
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind, &[]);
            pass.draw(0..3, 0..1);
        }
        queue.submit(Some(enc.finish()));
    }
}

impl SceneRenderer {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, inst: &SceneInstance) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("pmg-scene"),
            source: wgpu::ShaderSource::Wgsl(include_str!("scene.wgsl").into()),
        });

        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("pmg-scene-bind"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: wgpu::BufferSize::new(UNIFORM_SIZE),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        // Images get their own group so the per-node uniform group stays
        // untouched: a still is one texture per node, not a shared atlas, and
        // giving it group 1 means switching image only rebinds one group.
        let image_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("pmg-scene-image-bind"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        // TWO pipeline layouts, not one shared one.
        //
        // A pipeline layout declares what every draw using it must bind. Adding
        // the image group to the shared layout made box, text and unknown draws
        // fail validation with "expects a BindGroup to be set at index 1" —
        // they have no image to bind and should not have to invent one. Only
        // the image pipeline declares group 1.
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("pmg-scene-layout"),
            bind_group_layouts: &[Some(&bind_layout)],
            immediate_size: 0,
        });
        let image_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("pmg-scene-image-layout"),
                bind_group_layouts: &[Some(&bind_layout), Some(&image_layout)],
                immediate_size: 0,
            });

        // Premultiplied source-over. The shader already premultiplied, so the
        // source factor is One rather than SrcAlpha — using SrcAlpha here would
        // multiply by alpha twice and darken every edge.
        let blend = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
        };

        let make =
            |vs: &str, fs: &str, buffers: &[Option<wgpu::VertexBufferLayout>], label: &str| {
                device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some(label),
                    layout: Some(&layout),
                    vertex: wgpu::VertexState {
                        module: &shader,
                        entry_point: Some(vs),
                        buffers,
                        compilation_options: Default::default(),
                    },
                    fragment: Some(wgpu::FragmentState {
                        module: &shader,
                        entry_point: Some(fs),
                        targets: &[Some(wgpu::ColorTargetState {
                            format: COMPOSITE_FORMAT,
                            blend: Some(blend),
                            write_mask: wgpu::ColorWrites::ALL,
                        })],
                        compilation_options: Default::default(),
                    }),
                    primitive: wgpu::PrimitiveState {
                        topology: wgpu::PrimitiveTopology::TriangleList,
                        cull_mode: None,
                        ..Default::default()
                    },
                    depth_stencil: None,
                    // NEVER MSAA — sample patterns are unspecified across backends.
                    multisample: wgpu::MultisampleState {
                        count: 1,
                        ..Default::default()
                    },
                    multiview_mask: None,
                    cache: None,
                })
            };

        let glyph_layout = wgpu::VertexBufferLayout {
            array_stride: 16,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x2,
                    offset: 0,
                    shader_location: 0,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x2,
                    offset: 8,
                    shader_location: 1,
                },
            ],
        };

        let pipeline_box = make("vs_box", "fs_box", &[], "pmg-scene-box");
        let pipeline_unknown = make("vs_box", "fs_unknown", &[], "pmg-scene-unknown");
        let pipeline_image = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("pmg-scene-image"),
            layout: Some(&image_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_box"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_image"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: COMPOSITE_FORMAT,
                    blend: Some(blend),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let pipeline_text = make(
            "vs_text",
            "fs_text",
            &[Some(glyph_layout)],
            "pmg-scene-text",
        );

        // Upload the coverage atlas. A 1x1 transparent texture stands in when a
        // document has no text, so the bind group layout stays uniform.
        let (aw, ah) = if inst.atlas.is_empty() {
            (1, 1)
        } else {
            (inst.atlas.width, inst.atlas.height)
        };
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("pmg-scene-atlas"),
            size: wgpu::Extent3d {
                width: aw,
                height: ah,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let data: &[u8] = if inst.atlas.is_empty() {
            &[0u8]
        } else {
            &inst.atlas.bytes
        };
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(aw),
                rows_per_image: Some(ah),
            },
            wgpu::Extent3d {
                width: aw,
                height: ah,
                depth_or_array_layers: 1,
            },
        );

        // Every field stated. A filtering default is exactly the kind of thing
        // that differs quietly between backends.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("pmg-scene-sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            lod_min_clamp: 0.0,
            lod_max_clamp: 0.0,
            compare: None,
            anisotropy_clamp: 1,
            border_color: None,
        });

        // Its own sampler, stated field by field like every other one — every
        // sampler field is explicit so no default can differ between backends
        // (parity rule 4).
        let image_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("pmg-scene-image-sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            lod_min_clamp: 0.0,
            lod_max_clamp: 0.0,
            compare: None,
            anisotropy_clamp: 1,
            border_color: None,
        });

        // Textures are kept, not just their bind groups: a clip overwrites its
        // texture every frame, and a bind group alone gives no handle to write
        // through. See `update_image`.
        let (image_texs, image_binds): (Vec<_>, Vec<_>) = inst
            .images
            .iter()
            .map(|img| {
                let tex = device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("pmg-scene-image"),
                    size: wgpu::Extent3d {
                        width: img.width,
                        height: img.height,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    // Rgba8Unorm, never Rgba8UnormSrgb: no automatic colour
                    // conversion anywhere in this pipeline (parity rule 3).
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                });
                queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &tex,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    &img.rgba,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(img.width * 4),
                        rows_per_image: Some(img.height),
                    },
                    wgpu::Extent3d {
                        width: img.width,
                        height: img.height,
                        depth_or_array_layers: 1,
                    },
                );
                let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
                let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("pmg-scene-image-bind"),
                    layout: &image_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(&view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Sampler(&image_sampler),
                        },
                    ],
                });
                ((tex, img.width, img.height), bind)
            })
            .unzip();

        Self {
            pipeline_box,
            pipeline_text,
            pipeline_unknown,
            pipeline_image,
            image_texs,
            image_binds,
            bind_layout,
            sampler,
            atlas_view: tex.create_view(&wgpu::TextureViewDescriptor::default()),
            atlas_size: (aw, ah),
        }
    }

    /// Overwrite one image slot's texture — how a clip gets on air.
    ///
    /// **Premultiplied RGBA8, exactly the slot's size.** A frame of the wrong
    /// size is refused rather than stretched or partially written: a clip whose
    /// dimensions changed mid-playback is a bug upstream, and painting three
    /// quarters of a frame over the last one would hide it behind something that
    /// merely looks like tearing.
    ///
    /// Returns whether the write happened. Still no I/O and no platform — this
    /// is a queue write, so the crate compiles to WASM exactly as before.
    #[must_use]
    pub fn update_image(
        &self,
        queue: &wgpu::Queue,
        slot: usize,
        w: u32,
        h: u32,
        rgba: &[u8],
    ) -> bool {
        let Some((tex, tw, th)) = self.image_texs.get(slot) else {
            return false;
        };
        if w != *tw || h != *th || rgba.len() != (w as usize * h as usize * 4) {
            return false;
        }
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * 4),
                rows_per_image: Some(h),
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        true
    }

    /// Draw one frame.
    ///
    /// `frame` is `apply_tracks`' output — one entry per node, in the same
    /// order. Records and submits; does not read back.
    ///
    /// Clears the target first. Equivalent to [`render_with`] with
    /// [`Load::Clear`], and correct only for a single-layer output.
    pub fn render(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        inst: &SceneInstance,
        frame: &[FrameNode],
        target: &wgpu::TextureView,
    ) {
        self.render_with(device, queue, inst, frame, target, Load::Clear)
    }

    /// Draw one frame, choosing whether to clear the target or composite over
    /// what is already there.
    ///
    /// **A layer stack needs this.** The bottom layer clears; every layer above
    /// it uses [`Load::Over`]. Clearing unconditionally — which this renderer
    /// did until layers existed — means the last layer drawn is the only one
    /// visible, and the symptom is a Bug that silently erases the Lower Third
    /// under it rather than any kind of error.
    #[allow(clippy::too_many_arguments)]
    pub fn render_with(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        inst: &SceneInstance,
        frame: &[FrameNode],
        target: &wgpu::TextureView,
        load: Load,
    ) {
        let align = device.limits().min_uniform_buffer_offset_alignment as u64;
        let stride = UNIFORM_SIZE.div_ceil(align) * align;

        // One uniform slot per node, written in one go. Dynamic offsets then
        // let a single bind group serve every draw — a bind group per node
        // would be an allocation per node per frame.
        let mut ubytes = vec![0u8; (stride as usize) * inst.nodes.len().max(1)];
        let mut glyph_verts: Vec<f32> = Vec::new();
        let mut glyph_ranges: Vec<(u32, u32)> = Vec::new();

        // Compose each node's affine with its ancestors', so animating a parent
        // carries its children. Shared with the hit test (crate::interact) so
        // where a node draws and where it is clickable are one computation.
        let world = crate::interact::world_transforms(inst, frame);

        // Scissor per node: the intersection of every clipping ancestor.
        //
        // `clip` was declared by the scene format and ignored by this renderer
        // until 2026-08-16. It stopped being cosmetic with the live rebind: a
        // band that grows to fit new text must REVEAL that text as it grows,
        // and without clipping the new line is simply drawn at its final width
        // outside a box that has not got there yet.
        //
        // Shared with the hit test (crate::interact), which honours these
        // rects so a click on a pixel the clip removed does not select the
        // node whose pixels are not there.
        let clips = crate::interact::clip_rects(inst, frame, &world);

        for (i, n) in inst.nodes.iter().enumerate() {
            let f = frame.get(i).copied().unwrap_or(FrameNode {
                transform: n.base_transform,
                opacity: n.opacity,
                rect: None,
            });
            let rect = f.rect.unwrap_or(n.rect);
            let (m, t) = world[i];
            // Linear light, converted ONCE here on the CPU. Doing it in the
            // shader would repeat it per pixel for a value that is per node.
            let color = crate::color_to_linear(n.fill.map(|c| c.rgba).unwrap_or([1.0; 4]));

            let u = NodeUniform {
                rect: [rect.x, rect.y, rect.w, rect.h],
                color,
                xform: m,
                xform_t: t,
                radius: n.radius,
                opacity: f.opacity,
                canvas: [inst.canvas_w as f32, inst.canvas_h as f32],
                // One design-space unit per pixel. The corpus renders at design
                // resolution, so this is 1 — but it is a uniform rather than a
                // constant because playout at 2160 from a 1080 design changes it,
                // and because it replaces fwidth().
                px_per_unit: 1.0,
                _pad: 0.0,
                uv: match &n.content {
                    RenderContent::Image(img) => img.uv,
                    _ => [0.0, 0.0, 1.0, 1.0],
                },
            };
            let off = (stride as usize) * i;
            ubytes[off..off + UNIFORM_SIZE as usize].copy_from_slice(&uniform_bytes(&u));

            // Build glyph quads in the same pass.
            let start = (glyph_verts.len() / 4) as u32;
            if let RenderContent::Text(glyphs) = &n.content {
                let (aw, ah) = (self.atlas_size.0 as f32, self.atlas_size.1 as f32);
                for g in glyphs {
                    // Outset every glyph quad by half a pixel, with the UV rect
                    // extended to match.
                    //
                    // A run animated by its transform lands these quads at
                    // fractional positions, and the bilinearly-reconstructed
                    // coverage extends half a texel past the bitmap edge — but
                    // the rasterizer only shades pixels whose centres the quad
                    // reaches, so a quad cut exactly at the bitmap edge
                    // truncated that tail. Spike S2 measured the result as a
                    // 0.34 px stutter: edge columns snapped in and out as the
                    // quad edge crossed their pixel centres, the same defect
                    // the box path had from its own tight quad.
                    //
                    // Half a pixel is exact, not approximate: the atlas packer
                    // keeps a 1-texel zero border around every glyph (PAD in
                    // pack_atlas), and interpolated coverage fades to zero at
                    // that border's centre — precisely where this quad ends.
                    // Extending further would read past the pad into a
                    // neighbouring glyph; extending less re-truncates the tail.
                    let (x0, y0) = (g.x - 0.5, g.y - 0.5);
                    let (x1, y1) = (g.x + g.w + 0.5, g.y + g.h + 0.5);
                    let (u0, v0) = ((g.u as f32 - 0.5) / aw, (g.v as f32 - 0.5) / ah);
                    let (u1, v1) = (
                        ((g.u + g.uw) as f32 + 0.5) / aw,
                        ((g.v + g.uh) as f32 + 0.5) / ah,
                    );
                    for (px, py, tu, tv) in [
                        (x0, y0, u0, v0),
                        (x1, y0, u1, v0),
                        (x0, y1, u0, v1),
                        (x0, y1, u0, v1),
                        (x1, y0, u1, v0),
                        (x1, y1, u1, v1),
                    ] {
                        glyph_verts.extend_from_slice(&[px, py, tu, tv]);
                    }
                }
            }
            let end = (glyph_verts.len() / 4) as u32;
            glyph_ranges.push((start, end));
        }

        let ubuf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("pmg-scene-uniforms"),
            size: ubytes.len() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&ubuf, 0, &ubytes);

        let vbytes: Vec<u8> = glyph_verts.iter().flat_map(|f| f.to_le_bytes()).collect();
        let vbuf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("pmg-scene-glyphs"),
            size: vbytes.len().max(4) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        if !vbytes.is_empty() {
            queue.write_buffer(&vbuf, 0, &vbytes);
        }

        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("pmg-scene-bindgroup"),
            layout: &self.bind_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &ubuf,
                        offset: 0,
                        size: wgpu::BufferSize::new(UNIFORM_SIZE),
                    }),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&self.atlas_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });

        let mut enc = device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("pmg-scene-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: match load {
                            // Transparent black. A keyed graphic is alpha over
                            // nothing; anything else would fold a background
                            // into the key.
                            Load::Clear => wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            // Premultiplied source-over against what is already
                            // in the target — the layer below.
                            Load::Over => wgpu::LoadOp::Load,
                        },
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_vertex_buffer(0, vbuf.slice(..));

            // Painter's algorithm, tree order.
            let (cw, ch) = (inst.canvas_w, inst.canvas_h);
            let mut scissor_set = false;
            for (i, n) in inst.nodes.iter().enumerate() {
                let offset = (stride as usize * i) as u32;

                match clips[i] {
                    Some([x0, y0, x1, y1]) => {
                        // Clamped to the target, and skipped entirely when the
                        // rect has collapsed — a zero-width scissor is a
                        // validation error, and a box mid-morph can genuinely
                        // be zero wide.
                        let x = x0.max(0.0).min(cw as f32) as u32;
                        let y = y0.max(0.0).min(ch as f32) as u32;
                        let w = (x1.min(cw as f32) - x0.max(0.0)).max(0.0) as u32;
                        let h = (y1.min(ch as f32) - y0.max(0.0)).max(0.0) as u32;
                        if w == 0 || h == 0 {
                            continue;
                        }
                        pass.set_scissor_rect(x, y, w, h);
                        scissor_set = true;
                    }
                    None if scissor_set => {
                        pass.set_scissor_rect(0, 0, cw, ch);
                        scissor_set = false;
                    }
                    None => {}
                }

                match &n.content {
                    RenderContent::Unknown => {
                        pass.set_pipeline(&self.pipeline_unknown);
                        pass.set_bind_group(0, &bind, &[offset]);
                        pass.draw(0..6, 0..1);
                    }
                    RenderContent::Box => {
                        // A box with no fill is a pure layout container; drawing
                        // it would paint an opaque white rectangle over
                        // everything behind it.
                        if n.fill.is_some() {
                            pass.set_pipeline(&self.pipeline_box);
                            pass.set_bind_group(0, &bind, &[offset]);
                            pass.draw(0..6, 0..1);
                        }
                    }
                    RenderContent::Image(img) => {
                        if let Some(b) = self.image_binds.get(img.image) {
                            pass.set_pipeline(&self.pipeline_image);
                            pass.set_bind_group(0, &bind, &[offset]);
                            pass.set_bind_group(1, b, &[]);
                            pass.draw(0..6, 0..1);
                        }
                    }
                    RenderContent::Text(_) => {
                        let (s, e) = glyph_ranges[i];
                        if e > s {
                            pass.set_pipeline(&self.pipeline_text);
                            pass.set_bind_group(0, &bind, &[offset]);
                            pass.draw(s..e, 0..1);
                        }
                    }
                }
            }
        }
        queue.submit(Some(enc.finish()));
    }
}

/// A frame buffer that composites in linear and encodes on the way out.
///
/// The pairing every caller of [`SceneRenderer`] needs: a linear float texture
/// to draw the layer stack into, and an 8-bit one to read back. Handed out as
/// one thing because they are only ever correct together — a caller that
/// renders into the 8-bit texture directly gets gamma-space compositing back,
/// and the symptom is antialiased edges too dark against a dark key, which
/// reads as a font problem rather than a pipeline one.
pub struct LinearFrame {
    linear: wgpu::Texture,
    linear_view: wgpu::TextureView,
    output: wgpu::Texture,
    output_view: wgpu::TextureView,
    encoder: Encoder,
}

impl LinearFrame {
    pub fn new(device: &wgpu::Device, width: u32, height: u32) -> Self {
        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let make = |label, format, usage| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let linear = make(
            "pmg-linear",
            crate::COMPOSITE_FORMAT,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        );
        let output = make(
            "pmg-output",
            crate::TARGET_FORMAT,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        Self {
            linear_view: linear.create_view(&Default::default()),
            output_view: output.create_view(&Default::default()),
            encoder: Encoder::new(device),
            linear,
            output,
        }
    }

    /// Where layers draw. Linear, premultiplied.
    pub fn view(&self) -> &wgpu::TextureView {
        &self.linear_view
    }

    /// The encoded frame. Call [`Self::encode`] first.
    pub fn output(&self) -> &wgpu::Texture {
        &self.output
    }

    /// Once per frame, after the last layer and before reading back.
    pub fn encode(&self, device: &wgpu::Device, queue: &wgpu::Queue) {
        self.encoder
            .encode(device, queue, &self.linear_view, &self.output_view);
    }

    /// Clear the composite to transparent black.
    ///
    /// A keyed graphic is alpha over nothing; clearing to anything else folds a
    /// background into the key.
    pub fn clear(&self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let mut enc = device.create_command_encoder(&Default::default());
        {
            let _ = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("pmg-linear-clear"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.linear_view,
                    resolve_target: None,
                    depth_slice: None,
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
        queue.submit(Some(enc.finish()));
    }

    /// Unused today, kept so `linear` is not a dead field to the compiler.
    pub fn linear(&self) -> &wgpu::Texture {
        &self.linear
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interact::{affine, compose};
    use pmg_layout::Rect;
    use pmg_scene::doc::Transform2D;

    fn rect() -> Rect {
        Rect {
            x: 100.0,
            y: 200.0,
            w: 400.0,
            h: 100.0,
        }
    }

    #[test]
    fn an_identity_transform_is_the_identity() {
        let (m, t) = affine(&Transform2D::default(), &rect());
        assert_eq!(m, [1.0, 0.0, 0.0, 1.0]);
        assert!(
            t[0].abs() < 1e-4 && t[1].abs() < 1e-4,
            "identity translated: {t:?}"
        );
    }

    #[test]
    fn translation_is_applied_verbatim() {
        let t = Transform2D {
            translate: [-900.0, 12.0],
            ..Default::default()
        };
        let (_, tr) = affine(&t, &rect());
        assert!((tr[0] + 900.0).abs() < 1e-4);
        assert!((tr[1] - 12.0).abs() < 1e-4);
    }

    #[test]
    fn scale_pivots_on_the_anchor_not_the_origin() {
        // A lower third anchored at its left edge must hinge there — scaling
        // about the origin would slide it across the screen instead.
        let r = rect();
        let t = Transform2D {
            scale: [0.0, 1.0],
            anchor: [0.0, 0.5],
            ..Default::default()
        };
        let (m, tr) = affine(&t, &r);
        // The anchor point maps to itself under the full affine.
        let px = r.x + r.w * 0.0;
        let py = r.y + r.h * 0.5;
        let out_x = m[0] * px + m[2] * py + tr[0];
        let out_y = m[1] * px + m[3] * py + tr[1];
        assert!((out_x - px).abs() < 1e-3, "anchor x moved: {out_x} vs {px}");
        assert!((out_y - py).abs() < 1e-3, "anchor y moved: {out_y} vs {py}");
    }

    #[test]
    fn composing_with_an_identity_parent_changes_nothing() {
        let child = affine(
            &Transform2D {
                translate: [10.0, 20.0],
                ..Default::default()
            },
            &rect(),
        );
        let identity = ([1.0, 0.0, 0.0, 1.0], [0.0, 0.0]);
        assert_eq!(compose(identity, child), child);
    }

    #[test]
    fn a_parents_translation_carries_its_child() {
        // The bug this exists to prevent: the band slid left and the text
        // stayed, which looked like a clipping bug and was a missing multiply.
        let parent = affine(
            &Transform2D {
                translate: [-900.0, 0.0],
                ..Default::default()
            },
            &rect(),
        );
        let child = affine(&Transform2D::default(), &rect());
        let (m, t) = compose(parent, child);

        let p = [100.0f32, 200.0f32];
        let out_x = m[0] * p[0] + m[2] * p[1] + t[0];
        assert!(
            (out_x - (p[0] - 900.0)).abs() < 1e-3,
            "child did not move with its parent: {out_x}"
        );
    }

    #[test]
    fn the_uniform_block_matches_the_shader() {
        // If these drift, the shader reads garbage and the failure looks like a
        // rendering bug rather than a layout mismatch.
        assert_eq!(std::mem::size_of::<NodeUniform>() as u64, UNIFORM_SIZE);
        assert_eq!(UNIFORM_SIZE % 16, 0, "uniform must be 16-byte aligned");
    }
}
