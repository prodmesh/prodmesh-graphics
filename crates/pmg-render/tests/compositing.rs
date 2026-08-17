//! Layer compositing: the bottom layer clears, everything above it draws over.
//!
//! This exists because the renderer used to clear unconditionally. With one
//! layer that is invisible; with two, the top layer silently erased the one
//! beneath it — a Bug wiping the Lower Third under it, with no error anywhere.
//! The state report said both were live, because they were. Only the pixels
//! disagreed.

use pmg_render::scene::{GlyphSource, RunGlyph, SceneInstance};
use pmg_render::{Load, SceneRenderer, TARGET_FORMAT};
use pmg_scene::doc::{FontSpec, SceneDoc};

const W: u32 = 256;
const H: u32 = 256;

/// No text in these scenes; boxes are enough to see who covered whom.
struct NoGlyphs;
impl GlyphSource for NoGlyphs {
    fn run(
        &mut self,
        _text: &str,
        _font: &FontSpec,
        _max_width: Option<f32>,
        _max_lines: usize,
        _align: pmg_scene::doc::Align,
    ) -> Option<(Vec<RunGlyph>, f32)> {
        None
    }
}

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
}

fn gpu() -> Option<Gpu> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        force_fallback_adapter: false,
        compatible_surface: None,
        apply_limit_buckets: false,
    }))
    .ok()?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("pmg-compositing-test"),
        required_features: wgpu::Features::empty(),
        required_limits:
            wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits()),
        memory_hints: Default::default(),
        experimental_features: Default::default(),
        trace: Default::default(),
    }))
    .ok()?;
    Some(Gpu { device, queue })
}

/// A scene with one opaque bar 64 px wide and full height, offset from the left
/// by `x` using the root's padding.
///
/// Built from the JSON mirror rather than by hand so it exercises the same
/// deserializer a real template goes through.
fn bar(x: f32, colour: [f32; 4]) -> SceneDoc {
    let json = format!(
        r#"{{
          "format": "pmg.scene",
          "format_version": {{ "major": 1, "minor": 0 }},
          "min_reader": 1,
          "id": "01J8ZKCOMPOSITINGTEST001",
          "name": "bar",
          "canvas": {{ "width": {W}, "height": {H} }},
          "fonts": [],
          "fields": [],
          "assets": [],
          "timelines": [],
          "root": {{
            "id": "root",
            "kind": "box",
            "layout": {{
              "direction": "row",
              "justify": "start",
              "align": "start",
              "width": {{ "Px": {W}.0 }},
              "height": {{ "Px": {H}.0 }},
              "padding": {{ "top": 0.0, "right": 0.0, "bottom": 0.0, "left": {x:?} }}
            }},
            "children": [{{
              "id": "bar",
              "kind": "box",
              "layout": {{ "width": {{ "Px": 64.0 }}, "height": {{ "Px": {H}.0 }} }},
              "style": {{ "fill": {{ "rgba": {colour:?} }}, "opacity": 1.0 }}
            }}]
          }}
        }}"#
    );
    serde_json::from_str(&json).unwrap_or_else(|e| panic!("fixture scene: {e}\n{json}"))
}

fn instance(doc: &SceneDoc) -> SceneInstance {
    let solved = pmg_layout::solve(
        doc,
        &Default::default(),
        &pmg_layout::StubMeasure { advance_ratio: 0.5 },
    )
    .expect("layout solves");
    SceneInstance::build(doc, &solved, &mut NoGlyphs)
}

fn read_back(g: &Gpu, tex: &wgpu::Texture) -> Vec<u8> {
    let bpr = W * 4;
    let buf = g.device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: (bpr * H) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = g.device.create_command_encoder(&Default::default());
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buf,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bpr),
                rows_per_image: Some(H),
            },
        },
        wgpu::Extent3d {
            width: W,
            height: H,
            depth_or_array_layers: 1,
        },
    );
    g.queue.submit(Some(enc.finish()));
    let slice = buf.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    g.device.poll(wgpu::PollType::wait_indefinitely()).ok();
    rx.recv().unwrap().unwrap();
    let out = slice.get_mapped_range().unwrap().to_vec();
    buf.unmap();
    out
}

/// Alpha at the middle row, at x.
fn alpha_at(px: &[u8], x: u32) -> u8 {
    px[(((H / 2) * W + x) * 4 + 3) as usize]
}

#[test]
fn a_second_layer_draws_over_the_first_without_erasing_it() {
    let Some(g) = gpu() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };

    // Two bars that do not overlap: one at x=16, one at x=160.
    let lower = bar(16.0, [1.0, 0.0, 0.0, 1.0]);
    let upper = bar(160.0, [0.0, 0.0, 1.0, 1.0]);
    let (li, ui) = (instance(&lower), instance(&upper));

    let tex = g.device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width: W,
            height: H,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: TARGET_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = tex.create_view(&Default::default());

    let lr = SceneRenderer::new(&g.device, &g.queue, &li);
    let ur = SceneRenderer::new(&g.device, &g.queue, &ui);

    // Bottom layer clears; the one above composites over it.
    lr.render_with(&g.device, &g.queue, &li, &[], &view, Load::Clear);
    ur.render_with(&g.device, &g.queue, &ui, &[], &view, Load::Over);

    let px = read_back(&g, &tex);
    assert_eq!(alpha_at(&px, 48), 255, "the LOWER layer's bar must survive");
    assert_eq!(
        alpha_at(&px, 192),
        255,
        "the upper layer's bar must be drawn"
    );
    assert_eq!(
        alpha_at(&px, 112),
        0,
        "the gap between them stays transparent"
    );
}

#[test]
fn drawing_both_with_clear_loses_the_lower_layer() {
    // The bug, pinned as a fact rather than a memory: this is what the node did
    // before Load existed, and it is why `render` alone is documented as being
    // correct only for a single-layer output.
    let Some(g) = gpu() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let lower = bar(16.0, [1.0, 0.0, 0.0, 1.0]);
    let upper = bar(160.0, [0.0, 0.0, 1.0, 1.0]);
    let (li, ui) = (instance(&lower), instance(&upper));

    let tex = g.device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width: W,
            height: H,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: TARGET_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = tex.create_view(&Default::default());
    SceneRenderer::new(&g.device, &g.queue, &li).render(&g.device, &g.queue, &li, &[], &view);
    SceneRenderer::new(&g.device, &g.queue, &ui).render(&g.device, &g.queue, &ui, &[], &view);

    let px = read_back(&g, &tex);
    assert_eq!(
        alpha_at(&px, 48),
        0,
        "the lower layer is gone — this is the bug"
    );
    assert_eq!(alpha_at(&px, 192), 255);
}
