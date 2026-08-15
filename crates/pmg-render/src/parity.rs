//! The spike S1 parity corpus.
//!
//! Three figures which between them cover what a GPU actually does differently:
//!
//! | Figure | Tests |
//! |---|---|
//! | [`Figure::Triangle`] | Raster coverage rules, the vertex path |
//! | [`Figure::RoundedRect`] | **Our own analytic antialiasing** — the parity hazard |
//! | [`Figure::AlphaAtlas`] | Texture sampling — the glyph-atlas path |
//!
//! Real typography is deliberately absent. Glyphs are rasterized on the CPU in
//! `pmg-text`, so they are identical between targets by construction; the GPU's
//! contribution is sampling them, which [`Figure::AlphaAtlas`] measures without
//! dragging in the whole text stack. Typography quality is spike S2's job.
//!
//! Everything here is deterministic by construction — see the rules at the top
//! of `parity.wgsl`, each of which exists because breaking it would make the
//! two targets differ *by specification*.

use crate::TARGET_FORMAT;

/// Canvas size for every corpus render. Small enough to diff quickly, large
/// enough that an antialiased edge spans a meaningful number of pixels.
pub const SIZE: u32 = 512;

/// Uniform block. Must match `Uniforms` in `parity.wgsl` exactly.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct Uniforms {
    center: [f32; 2],
    half_extent: [f32; 2],
    color: [f32; 4],
    radius: f32,
    px_per_unit: f32,
    _pad: [f32; 2],
}

// std140-ish: 16-byte aligned, 48 bytes total.
const _: () = assert!(std::mem::size_of::<Uniforms>() == 48);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Figure {
    /// Stage A. A solid triangle — fixed vertices, flat colour.
    Triangle,
    /// Stage B. An analytically antialiased rounded rectangle. **The one that
    /// matters**: this is our AA math, and shared code is supposed to make it
    /// identical.
    RoundedRect,
    /// Stage C. A CPU-generated alpha bitmap sampled onto a quad — the
    /// glyph-atlas path, minus the text stack.
    AlphaAtlas,
}

impl Figure {
    pub const ALL: &'static [Figure] = &[Figure::Triangle, Figure::RoundedRect, Figure::AlphaAtlas];

    pub fn name(&self) -> &'static str {
        match self {
            Figure::Triangle => "triangle",
            Figure::RoundedRect => "rounded-rect",
            Figure::AlphaAtlas => "alpha-atlas",
        }
    }

    pub fn parse(s: &str) -> Option<Figure> {
        Figure::ALL.iter().copied().find(|f| f.name() == s)
    }
}

/// A CPU-generated alpha bitmap standing in for a glyph atlas.
///
/// Deliberately built from the same analytic-coverage math the renderer uses,
/// so its edges are soft in the way a rasterized glyph's are. Pure and
/// deterministic: identical bytes on both targets, which is the point — any
/// difference downstream is the GPU's sampling, not the source.
pub fn alpha_atlas(size: u32) -> Vec<u8> {
    let n = size as usize;
    let mut out = vec![0u8; n * n];
    let c = size as f32 * 0.5;
    let r = size as f32 * 0.34;
    for y in 0..n {
        for x in 0..n {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            // A ring, so the bitmap has two soft edges rather than one and the
            // sampler is exercised on both a rising and a falling gradient.
            let d = ((px - c).powi(2) + (py - c).powi(2)).sqrt();
            let outer = (0.5 - (d - r)).clamp(0.0, 1.0);
            let inner = (0.5 - (r * 0.55 - d)).clamp(0.0, 1.0);
            let a = (outer * inner).clamp(0.0, 1.0);
            out[y * n + x] = (a * 255.0).round() as u8;
        }
    }
    out
}

/// Everything needed to render the corpus, built once.
pub struct Corpus {
    pipeline_solid: wgpu::RenderPipeline,
    pipeline_rect: wgpu::RenderPipeline,
    pipeline_atlas: wgpu::RenderPipeline,
    bind_layout: wgpu::BindGroupLayout,
    uniform: wgpu::Buffer,
    atlas_view: wgpu::TextureView,
    sampler: wgpu::Sampler,
}

impl Corpus {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("pmg-parity"),
            source: wgpu::ShaderSource::Wgsl(include_str!("parity.wgsl").into()),
        });

        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("pmg-parity-bind"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
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

        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("pmg-parity-layout"),
            bind_group_layouts: &[Some(&bind_layout)],
            immediate_size: 0,
        });

        let make = |vs: &str, fs: &str, label: &str| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(vs),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fs),
                    // No blending. The corpus renders onto a cleared target and
                    // writes premultiplied values directly, so blend-equation
                    // differences are not part of the measurement.
                    targets: &[Some(wgpu::ColorTargetState {
                        format: TARGET_FORMAT,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    front_face: wgpu::FrontFace::Ccw,
                    // Culling off: winding is not what this measures, and a
                    // culled triangle would fail confusingly.
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: None,
                // NEVER MSAA. Sample patterns are unspecified across backends,
                // which makes it a parity hazard by construction (risk R1).
                multisample: wgpu::MultisampleState {
                    count: 1,
                    ..Default::default()
                },
                multiview_mask: None,
                cache: None,
            })
        };

        let pipeline_solid = make("vs_triangle", "fs_solid", "pmg-parity-solid");
        let pipeline_rect = make("vs_fullscreen", "fs_rounded_rect", "pmg-parity-rect");
        let pipeline_atlas = make("vs_fullscreen", "fs_atlas", "pmg-parity-atlas");

        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("pmg-parity-uniform"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Upload the CPU-generated atlas as R8Unorm.
        let atlas_size = 256u32;
        let atlas = alpha_atlas(atlas_size);
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("pmg-parity-atlas"),
            size: wgpu::Extent3d {
                width: atlas_size,
                height: atlas_size,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
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
            &atlas,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(atlas_size),
                rows_per_image: Some(atlas_size),
            },
            wgpu::Extent3d {
                width: atlas_size,
                height: atlas_size,
                depth_or_array_layers: 1,
            },
        );

        // EVERY field stated. Relying on a filtering default is exactly the kind
        // of thing that differs quietly between backends.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("pmg-parity-sampler"),
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

        Self {
            pipeline_solid,
            pipeline_rect,
            pipeline_atlas,
            bind_layout,
            uniform,
            atlas_view: tex.create_view(&wgpu::TextureViewDescriptor::default()),
            sampler,
        }
    }

    /// Record a render of `figure` into `target`.
    ///
    /// Records and submits; does not read back. Readback is the caller's
    /// problem precisely because how you await it differs by platform.
    pub fn render(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        figure: Figure,
        target: &wgpu::TextureView,
    ) {
        // One pixel expressed in the shader's [-1,1] space.
        let px_per_unit = 2.0 / SIZE as f32;

        let u = match figure {
            Figure::Triangle => Uniforms {
                center: [0.0, 0.0],
                half_extent: [0.0, 0.0],
                color: [0.20, 0.66, 1.0, 1.0],
                radius: 0.0,
                px_per_unit,
                _pad: [0.0, 0.0],
            },
            Figure::RoundedRect => Uniforms {
                center: [0.0, 0.0],
                half_extent: [0.62, 0.38],
                color: [1.0, 1.0, 1.0, 1.0],
                // Deliberately large: a generous corner radius means a long
                // curved antialiased edge, which is where a coverage difference
                // would show up most clearly.
                radius: 0.22,
                px_per_unit,
                _pad: [0.0, 0.0],
            },
            Figure::AlphaAtlas => Uniforms {
                center: [0.0, 0.0],
                half_extent: [0.0, 0.0],
                color: [1.0, 1.0, 1.0, 1.0],
                radius: 0.0,
                px_per_unit,
                _pad: [0.0, 0.0],
            },
        };
        queue.write_buffer(&self.uniform, 0, &uniform_bytes(&u));

        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("pmg-parity-bindgroup"),
            layout: &self.bind_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.uniform.as_entire_binding(),
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

        let (pipeline, verts) = match figure {
            Figure::Triangle => (&self.pipeline_solid, 3),
            Figure::RoundedRect => (&self.pipeline_rect, 6),
            Figure::AlphaAtlas => (&self.pipeline_atlas, 6),
        };

        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("pmg-parity-encoder"),
        });
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("pmg-parity-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        // Clear to transparent black. Anything else would fold a
                        // background into the alpha comparison.
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &bind, &[]);
            pass.draw(0..verts, 0..1);
        }
        queue.submit(Some(enc.finish()));
    }
}

/// Pack the uniform block to bytes by hand.
///
/// `bytemuck` would do this, but it is one more dependency in the pure set for
/// twelve floats — and doing it explicitly means the byte layout is visible next
/// to the WGSL struct it has to match.
fn uniform_bytes(u: &Uniforms) -> [u8; 48] {
    let mut buf = [0u8; 48];
    let fields = [
        u.center[0],
        u.center[1],
        u.half_extent[0],
        u.half_extent[1],
        u.color[0],
        u.color[1],
        u.color[2],
        u.color[3],
        u.radius,
        u.px_per_unit,
        u._pad[0],
        u._pad[1],
    ];
    for (i, v) in fields.iter().enumerate() {
        buf[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
    }
    buf
}

pub fn selftest() -> Result<(), String> {
    // The atlas is pure: same bytes every time, on every target. If this ever
    // fails, the "identical by construction" claim for glyph rasterization is
    // false and stage C measures nothing.
    let a = alpha_atlas(64);
    let b = alpha_atlas(64);
    if a != b {
        return Err("alpha_atlas is not deterministic".into());
    }
    if a.len() != 64 * 64 {
        return Err("alpha_atlas returned the wrong length".into());
    }
    // A ring must have both fully-opaque and fully-transparent pixels, and soft
    // ones in between — otherwise there is no edge for the sampler to blur.
    let soft = a.iter().filter(|v| **v > 8 && **v < 247).count();
    if soft < 100 {
        return Err(format!(
            "alpha_atlas has no soft edge ({soft} partial pixels)"
        ));
    }
    if !a.iter().any(|v| *v > 250) || !a.iter().any(|v| *v < 5) {
        return Err("alpha_atlas has no fully opaque or fully transparent region".into());
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

    #[test]
    fn figure_names_round_trip() {
        for f in Figure::ALL {
            assert_eq!(Figure::parse(f.name()), Some(*f));
        }
    }
}
