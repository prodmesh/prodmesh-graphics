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
use crate::TARGET_FORMAT;
use pmg_scene::doc::Transform2D;

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
}

const UNIFORM_SIZE: u64 = 80;
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
    ];
    for (i, v) in f.iter().enumerate() {
        buf[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
    }
    buf
}

/// Build the 2×3 affine for a node: scale and rotate about the anchor, then
/// translate.
///
/// The anchor is a fraction of the node's own rect, so a lower third anchored at
/// `[0, 0.5]` rotates about its left edge rather than its centre — which is what
/// a designer means by "hinge it on the left".
fn affine(t: &Transform2D, rect: &pmg_layout::Rect) -> ([f32; 4], [f32; 2]) {
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
///
/// Both are column-major 2x2 plus a translation, matching `mat2x2` in WGSL.
fn compose(outer: ([f32; 4], [f32; 2]), inner: ([f32; 4], [f32; 2])) -> ([f32; 4], [f32; 2]) {
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

/// Compiled pipelines and the uploaded atlas for one instance.
pub struct SceneRenderer {
    pipeline_box: wgpu::RenderPipeline,
    pipeline_text: wgpu::RenderPipeline,
    pipeline_unknown: wgpu::RenderPipeline,
    bind_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    atlas_view: wgpu::TextureView,
    atlas_size: (u32, u32),
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

        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("pmg-scene-layout"),
            bind_group_layouts: &[Some(&bind_layout)],
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
                            format: TARGET_FORMAT,
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

        Self {
            pipeline_box,
            pipeline_text,
            pipeline_unknown,
            bind_layout,
            sampler,
            atlas_view: tex.create_view(&wgpu::TextureViewDescriptor::default()),
            atlas_size: (aw, ah),
        }
    }

    /// Draw one frame.
    ///
    /// `frame` is `apply_tracks`' output — one entry per node, in the same
    /// order. Records and submits; does not read back.
    pub fn render(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        inst: &SceneInstance,
        frame: &[FrameNode],
        target: &wgpu::TextureView,
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
        // carries its children. Parents always precede children in `nodes`
        // (flatten is depth-first, pushing before recursing), so one forward
        // pass is enough — no second traversal, no recursion.
        let mut world: Vec<([f32; 4], [f32; 2])> = Vec::with_capacity(inst.nodes.len());
        for (i, n) in inst.nodes.iter().enumerate() {
            let f = frame.get(i).copied().unwrap_or(FrameNode {
                transform: n.base_transform,
                opacity: n.opacity,
            });
            let local = affine(&f.transform, &n.rect);
            world.push(match n.parent.and_then(|p| world.get(p).copied()) {
                Some(parent) => compose(parent, local),
                None => local,
            });
        }

        for (i, n) in inst.nodes.iter().enumerate() {
            let f = frame.get(i).copied().unwrap_or(FrameNode {
                transform: n.base_transform,
                opacity: n.opacity,
            });
            let (m, t) = world[i];
            let color = n.fill.map(|c| c.rgba).unwrap_or([1.0, 1.0, 1.0, 1.0]);

            let u = NodeUniform {
                rect: [n.rect.x, n.rect.y, n.rect.w, n.rect.h],
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
            };
            let off = (stride as usize) * i;
            ubytes[off..off + UNIFORM_SIZE as usize].copy_from_slice(&uniform_bytes(&u));

            // Build glyph quads in the same pass.
            let start = (glyph_verts.len() / 4) as u32;
            if let RenderContent::Text(glyphs) = &n.content {
                let (aw, ah) = (self.atlas_size.0 as f32, self.atlas_size.1 as f32);
                for g in glyphs {
                    let (x0, y0) = (g.x, g.y);
                    let (x1, y1) = (g.x + g.w, g.y + g.h);
                    let (u0, v0) = (g.u as f32 / aw, g.v as f32 / ah);
                    let (u1, v1) = ((g.u + g.uw) as f32 / aw, (g.v + g.uh) as f32 / ah);
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
                        // Transparent black. A keyed graphic is alpha over
                        // nothing; anything else would fold a background into
                        // the key.
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
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
            for (i, n) in inst.nodes.iter().enumerate() {
                let offset = (stride as usize * i) as u32;
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

#[cfg(test)]
mod tests {
    use super::*;
    use pmg_layout::Rect;

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
