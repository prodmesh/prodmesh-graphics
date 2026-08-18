//! Images: that they draw at all, and that `fit` crops the right axis.
//!
//! The fit maths is pure and testable without a GPU, so most of this file is.
//! The one GPU test exists because "the pixels arrived" is the claim, and a
//! unit test of the UV rect cannot make it.

use pmg_render::scene::fit_uv;
use pmg_scene::doc::Fit;

/// A 2:1 image in a 1:1 box, and the reverse.
#[test]
fn cover_crops_the_overflowing_axis_and_keeps_the_centre() {
    // Wide image, square box: crop left and right, keep full height.
    let uv = fit_uv(Fit::Cover, 200, 100, 100.0, 100.0);
    assert!(
        (uv[2] - 0.5).abs() < 1e-6,
        "half the width is sampled: {uv:?}"
    );
    assert!((uv[3] - 1.0).abs() < 1e-6, "all of the height is sampled");
    // Centred, so the crop takes equal bites off both sides rather than
    // anchoring to a corner.
    assert!((uv[0] - 0.25).abs() < 1e-6, "centred horizontally: {uv:?}");
    assert!((uv[1] - 0.0).abs() < 1e-6);

    // Tall image, square box: crop top and bottom.
    let uv = fit_uv(Fit::Cover, 100, 200, 100.0, 100.0);
    assert!(
        (uv[3] - 0.5).abs() < 1e-6,
        "half the height is sampled: {uv:?}"
    );
    assert!((uv[1] - 0.25).abs() < 1e-6, "centred vertically");
}

#[test]
fn a_matching_aspect_ratio_is_not_cropped_at_all() {
    let uv = fit_uv(Fit::Cover, 1920, 1080, 960.0, 540.0);
    assert!((uv[0]).abs() < 1e-6);
    assert!((uv[1]).abs() < 1e-6);
    assert!((uv[2] - 1.0).abs() < 1e-6);
    assert!((uv[3] - 1.0).abs() < 1e-6);
}

#[test]
fn stretch_samples_everything_and_lets_the_quad_distort_it() {
    let uv = fit_uv(Fit::Stretch, 200, 100, 100.0, 400.0);
    assert_eq!(uv, [0.0, 0.0, 1.0, 1.0]);
}

#[test]
fn a_degenerate_size_does_not_divide_by_zero() {
    // A box that laid out to nothing, or an image that decoded to nothing.
    // Returning the whole source beats a NaN reaching the shader, where it
    // would sample garbage rather than fail.
    assert_eq!(fit_uv(Fit::Cover, 0, 0, 100.0, 100.0), [0.0, 0.0, 1.0, 1.0]);
    assert_eq!(fit_uv(Fit::Cover, 100, 100, 0.0, 0.0), [0.0, 0.0, 1.0, 1.0]);
}

// ---------------------------------------------------------------------------

/// The pixels actually arrive, and premultiplied alpha composites correctly.
#[test]
fn an_image_draws_and_its_alpha_composites() {
    let Some((device, queue)) = gpu() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };

    // Four 32x32 quadrants rather than a 2x2 image.
    //
    // A 2x2 texture stretched to 64x64 is almost entirely BILINEAR BLEND: only
    // the exact texel centres are pure, so sampling "the top-left quadrant"
    // actually measures interpolation between all four texels. The first
    // version of this test did exactly that and read a transparent corner as
    // [60,0,0,60] — a real property of linear filtering, not a renderer bug.
    // Big quadrants put every sample far from an edge.
    let mut png_bytes = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut png_bytes, 64, 64);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().unwrap();
        let mut px = Vec::with_capacity(64 * 64 * 4);
        for y in 0..64 {
            for x in 0..64 {
                // TL opaque red · TR half-alpha red · BL transparent · BR red
                let (r, a) = match (x < 32, y < 32) {
                    (true, true) => (255u8, 255u8),
                    (false, true) => (255, 128),
                    (true, false) => (255, 0),
                    (false, false) => (255, 255),
                };
                px.extend_from_slice(&[r, 0, 0, a]);
            }
        }
        w.write_image_data(&px).unwrap();
    }
    let decoded = pmg_media::decode_image(&png_bytes).expect("decode");
    // Premultiplied at decode: the half-alpha quadrant's red is 128, not 255.
    let tr = ((10 * 64) + 40) * 4; // inside the top-right quadrant
    assert_eq!(decoded.rgba[tr], 128, "the decoder must premultiply");

    let doc: pmg_scene::doc::SceneDoc = serde_json::from_str(&format!(
        r#"{{
          "format": "pmg.scene",
          "format_version": {{ "major": 1, "minor": 0 }},
          "min_reader": 1,
          "id": "01J8ZKIMAGETEST000000001",
          "name": "image",
          "canvas": {{ "width": 64, "height": 64 }},
          "fonts": [], "fields": [], "timelines": [],
          "assets": [{{ "key": "bg", "hash": "blake3:{}", "kind": "image" }}],
          "root": {{
            "id": "root", "kind": "box",
            "layout": {{ "width": {{ "Px": 64.0 }}, "height": {{ "Px": 64.0 }} }},
            "children": [{{
              "id": "pic", "kind": "image",
              "layout": {{ "width": {{ "Px": 64.0 }}, "height": {{ "Px": 64.0 }} }},
              "content": {{ "kind": "image", "asset_key": "bg", "fit": "stretch" }}
            }}]
          }}
        }}"#,
        "0".repeat(64)
    ))
    .expect("fixture");

    struct OneImage(pmg_media::Decoded);
    impl pmg_render::scene::ImageSource for OneImage {
        fn image(&mut self, _k: &str) -> Option<(u32, u32, std::sync::Arc<Vec<u8>>)> {
            Some((
                self.0.width,
                self.0.height,
                std::sync::Arc::new(self.0.rgba.clone()),
            ))
        }
    }
    struct NoGlyphs;
    impl pmg_render::scene::GlyphSource for NoGlyphs {
        fn run(
            &mut self,
            _t: &str,
            _f: &pmg_scene::doc::FontSpec,
            _w: Option<f32>,
            _l: usize,
            _a: pmg_scene::doc::Align,
        ) -> Option<(Vec<pmg_render::scene::RunGlyph>, f32)> {
            None
        }
    }

    let solved = pmg_layout::solve(
        &doc,
        &Default::default(),
        &pmg_layout::StubMeasure { advance_ratio: 0.5 },
    )
    .expect("layout");
    let inst = pmg_render::scene::SceneInstance::build_with(
        &doc,
        &solved,
        &mut NoGlyphs,
        &mut OneImage(decoded),
    );
    assert_eq!(inst.images.len(), 1, "the image was collected");
    assert!(
        !inst.degraded,
        "a staged image must not mark the scene degraded"
    );

    let px = render(&device, &queue, &inst);
    // Top-left quadrant is the opaque red texel.
    let at = |x: u32, y: u32| {
        let i = ((y * 64 + x) * 4) as usize;
        [px[i], px[i + 1], px[i + 2], px[i + 3]]
    };
    assert_eq!(at(8, 8), [255, 0, 0, 255], "opaque red quadrant");
    // Bottom-left is fully transparent: premultiplied, so all zero.
    assert_eq!(
        at(8, 56),
        [0, 0, 0, 0],
        "transparent quadrant stays transparent"
    );
    // Top-right is half alpha; premultiplied red is ~128 with alpha ~128.
    let hr = at(56, 8);
    assert!(
        (hr[0] as i32 - 128).abs() <= 2,
        "premultiplied red ~128, got {hr:?}"
    );
    assert!((hr[3] as i32 - 128).abs() <= 2, "alpha ~128, got {hr:?}");
}

// ---------------------------------------------------------------------------

fn gpu() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        force_fallback_adapter: false,
        compatible_surface: None,
        apply_limit_buckets: false,
    }))
    .ok()?;
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("pmg-image-test"),
        required_features: wgpu::Features::empty(),
        required_limits:
            wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits()),
        memory_hints: Default::default(),
        experimental_features: Default::default(),
        trace: Default::default(),
    }))
    .ok()
}

fn render(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    inst: &pmg_render::scene::SceneInstance,
) -> Vec<u8> {
    // Linear composite plus encode, the same path the node takes.
    let fb = pmg_render::draw::LinearFrame::new(device, 64, 64);
    let view = fb.view().clone();
    let r = pmg_render::SceneRenderer::new(device, queue, inst);
    r.render(device, queue, inst, &[], &view);
    fb.encode(device, queue);

    let buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 64 * 64 * 4,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&Default::default());
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: fb.output(),
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buf,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(256),
                rows_per_image: Some(64),
            },
        },
        wgpu::Extent3d {
            width: 64,
            height: 64,
            depth_or_array_layers: 1,
        },
    );
    queue.submit(Some(enc.finish()));
    let slice = buf.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    device.poll(wgpu::PollType::wait_indefinitely()).ok();
    rx.recv().unwrap().unwrap();
    let out = slice.get_mapped_range().unwrap().to_vec();
    buf.unmap();
    out
}
