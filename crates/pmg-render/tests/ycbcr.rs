//! The GPU's v210 encode against the reference in `pmg_color::v210`.
//!
//! One definition of the numbers, two executions of it — and this is the test
//! that keeps them one definition. Run on the node's own limits, so a pass that
//! only works on a generous device fails here rather than at boot.

use pmg_color::v210::{self, Levels};
use pmg_color::{Matrix, Range};
use pmg_render::ycbcr::{YcbcrEncoder, YcbcrTarget};

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
        label: Some("pmg-v210-test"),
        required_features: wgpu::Features::empty(),
        // The node's limits, deliberately.
        required_limits:
            wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits()),
        memory_hints: Default::default(),
        experimental_features: Default::default(),
        trace: Default::default(),
    }))
    .ok()?;
    Some(Gpu { device, queue })
}

/// A frame worth encoding: ramps of colour and coverage, the primaries, fully
/// transparent and fully opaque pixels, and values just either side of the
/// sRGB toe — everything a code value can go wrong on.
fn frame(w: u32, h: u32) -> Vec<f32> {
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    let mut seed = 0x9e37_79b9u32;
    let mut rnd = || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        (seed as f32) / (u32::MAX as f32)
    };
    for y in 0..h {
        for x in 0..w {
            let t = x as f32 / (w - 1) as f32;
            let px = match (y % 4, x % 7) {
                (0, _) => [t, t, t, 1.0],
                (1, 0) => [0.0; 4],
                (1, 1) => [1.0, 0.0, 0.0, 1.0],
                (1, 2) => [0.0, 1.0, 0.0, 1.0],
                (1, 3) => [0.0, 0.0, 1.0, 1.0],
                (1, 4) => [0.003, 0.0032, 0.0031, 1.0],
                _ => {
                    let a = rnd();
                    [rnd() * a, rnd() * a, rnd() * a, a]
                }
            };
            out.extend_from_slice(&px);
        }
    }
    out
}

fn upload(g: &Gpu, rgba: &[f32], w: u32, h: u32) -> wgpu::TextureView {
    // Rgba32Float rather than the composite's Rgba16Float, so the reference is
    // handed exactly the values the GPU reads. The shader only textureLoads, so
    // the format makes no other difference.
    let tex = g.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("linear"),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba32Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let bytes: Vec<u8> = rgba.iter().flat_map(|f| f.to_le_bytes()).collect();
    g.queue.write_texture(
        tex.as_image_copy(),
        &bytes,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(w * 16),
            rows_per_image: Some(h),
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    tex.create_view(&Default::default())
}

fn read(g: &Gpu, tex: &wgpu::Texture, row_bytes: usize, h: u32) -> Vec<u8> {
    let padded = row_bytes.div_ceil(256) * 256;
    let buf = g.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: (padded * h as usize) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = g.device.create_command_encoder(&Default::default());
    enc.copy_texture_to_buffer(
        tex.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buf,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded as u32),
                rows_per_image: Some(h),
            },
        },
        tex.size(),
    );
    g.queue.submit(Some(enc.finish()));
    let slice = buf.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    g.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    let mapped = slice.get_mapped_range().unwrap();
    let mut out = Vec::with_capacity(row_bytes * h as usize);
    for y in 0..h as usize {
        out.extend_from_slice(&mapped[y * padded..y * padded + row_bytes]);
    }
    out
}

/// Every code value of `got` within `tol` of `want`, block by block.
fn agree(got: &[u8], want: &[u8], tol: u32, what: &str) {
    assert_eq!(got.len(), want.len(), "{what}: sizes");
    let mut worst = 0;
    for (i, (g, w)) in got.chunks(16).zip(want.chunks(16)).enumerate() {
        let (gy, gc) = v210::unpack(g);
        let (wy, wc) = v210::unpack(w);
        for k in 0..6 {
            worst = worst.max(gy[k].abs_diff(wy[k]));
        }
        for k in 0..3 {
            worst = worst
                .max(gc[k].0.abs_diff(wc[k].0))
                .max(gc[k].1.abs_diff(wc[k].1));
        }
        assert!(
            worst <= tol,
            "{what}: block {i} differs by {worst} code values: gpu {:?} reference {:?}",
            (gy, gc),
            (wy, wc)
        );
    }
}

fn check(matrix: Matrix, range: Range, key_range: Range) {
    let Some(g) = gpu() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    // 1280 is not a multiple of 48, so the padding at the end of every row is
    // exercised too.
    let (w, h) = (1280, 8);
    let rgba = frame(w, h);
    let levels = Levels::new(matrix, range, key_range);
    let (want_fill, want_key) = v210::encode(&rgba, w, h, &levels);

    let view = upload(&g, &rgba, w, h);
    let target = YcbcrTarget::new(&g.device, w, h);
    YcbcrEncoder::new(&g.device).encode(&g.device, &g.queue, &view, &target, &levels);
    let fill = read(&g, target.fill(), target.row_bytes(), h);
    let key = read(&g, target.key(), target.row_bytes(), h);

    // One code value: pow() is not correctly rounded on any GPU, and a value
    // that lands within an ulp of .5 may round either way. S9's own tolerance
    // on a scope is two.
    agree(&fill, &want_fill, 1, "fill");
    // The key has no transcendental in it at all. Exact.
    agree(&key, &want_key, 0, "key");
}

#[test]
fn gpu_matches_the_reference_bt709_narrow() {
    check(Matrix::Bt709, Range::Narrow, Range::Narrow);
}

#[test]
fn gpu_matches_the_reference_bt601_full_with_a_narrow_key() {
    check(Matrix::Bt601, Range::Full, Range::Narrow);
}

#[test]
fn gpu_matches_the_reference_bt2020_narrow_with_a_full_key() {
    check(Matrix::Bt2020, Range::Narrow, Range::Full);
}
