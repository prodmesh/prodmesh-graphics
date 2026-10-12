//! The node's own YCbCr encode: linear composite → 10-bit 4:2:2 `v210` fill
//! and key, on the GPU (ADR 0012).
//!
//! Without it an SDI output is handed 8-bit BGRA and the card converts with its
//! own matrix and range — the output's colour configuration reaches the node
//! and changes nothing. This pass applies it: the configured matrix and fill
//! range, and the key's independent range with `Y = alpha` and neutral chroma.
//! It reads the LINEAR composite, so the 10 bits are real rather than 8 bits
//! spread out.
//!
//! On the GPU because a CPU-generated 1080p frame misses 59.94 (AGENTS.md); the
//! arithmetic is defined once, in `pmg_color::v210`, and this is tested
//! against it.

use pmg_color::v210::{v210_row_bytes, Levels};

/// Both `v210` planes for one output, as GPU textures: one `R32Uint` texel per
/// packed word, `row_bytes / 4` texels wide.
pub struct YcbcrTarget {
    pub width: u32,
    pub height: u32,
    fill: wgpu::Texture,
    key: wgpu::Texture,
    fill_view: wgpu::TextureView,
    key_view: wgpu::TextureView,
    uniform: wgpu::Buffer,
}

impl YcbcrTarget {
    pub fn new(device: &wgpu::Device, width: u32, height: u32) -> Self {
        let words = (v210_row_bytes(width) / 4) as u32;
        let plane = |label| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: words,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::R32Uint,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            })
        };
        let fill = plane("pmg-v210-fill");
        let key = plane("pmg-v210-key");
        let fill_view = fill.create_view(&Default::default());
        let key_view = key.create_view(&Default::default());
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("pmg-v210-levels"),
            size: 80,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            width,
            height,
            fill,
            key,
            fill_view,
            key_view,
            uniform,
        }
    }

    /// Bytes per row of each plane, as DeckLink lays a `v210` frame out.
    pub fn row_bytes(&self) -> usize {
        v210_row_bytes(self.width)
    }

    /// The fill plane, for readback.
    pub fn fill(&self) -> &wgpu::Texture {
        &self.fill
    }

    /// The key plane, for readback.
    pub fn key(&self) -> &wgpu::Texture {
        &self.key
    }
}

pub struct YcbcrEncoder {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
}

impl YcbcrEncoder {
    pub fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("pmg-v210"),
            source: wgpu::ShaderSource::Wgsl(include_str!("ycbcr.wgsl").into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("pmg-v210-bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
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
            ],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("pmg-v210-pl"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        // Integer targets cannot blend, and nothing here should: each texel
        // is a finished word.
        let plane = Some(wgpu::ColorTargetState {
            format: wgpu::TextureFormat::R32Uint,
            blend: None,
            write_mask: wgpu::ColorWrites::ALL,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("pmg-v210-pipe"),
            layout: Some(&pl),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_ycbcr"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_ycbcr"),
                targets: &[plane.clone(), plane],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        Self { pipeline, layout }
    }

    /// Encode `linear` — the composite, linear and premultiplied, the same
    /// size as `target` — into `target`'s fill and key planes.
    pub fn encode(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        linear: &wgpu::TextureView,
        target: &YcbcrTarget,
        levels: &Levels,
    ) {
        let l = levels;
        let floats: [f32; 16] = [
            l.kr, l.kb, l.y_off, l.y_scale, //
            l.c_scale, l.y_min, l.y_max, l.c_min, //
            l.c_max, l.k_off, l.k_scale, l.k_min, //
            l.k_max, 0.0, 0.0, 0.0,
        ];
        let mut bytes = Vec::with_capacity(80);
        for f in floats {
            bytes.extend_from_slice(&f.to_le_bytes());
        }
        for u in [target.width, target.height, 0, 0] {
            bytes.extend_from_slice(&u.to_le_bytes());
        }
        queue.write_buffer(&target.uniform, 0, &bytes);

        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("pmg-v210-bg"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: target.uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(linear),
                },
            ],
        });
        let attach = |view| {
            Some(wgpu::RenderPassColorAttachment {
                view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })
        };
        let mut enc = device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("pmg-v210-pass"),
                color_attachments: &[attach(&target.fill_view), attach(&target.key_view)],
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
