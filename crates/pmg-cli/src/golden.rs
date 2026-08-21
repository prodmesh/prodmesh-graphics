//! `pmgctl render-golden` and `pmgctl golden-diff` — the parity suite.
//!
//! Renders the corpus offscreen with the native wgpu backend and writes PNGs,
//! then compares a native directory against a WASM one.
//!
//! Two corpora, deliberately:
//!
//! - **`pmg_render::parity`** — three hand-drawn primitives covering what a GPU
//!   does differently at all: fill, our own antialiasing math, texture
//!   sampling. They gated M0 and they still run first.
//! - **`pmg_render::corpus`** — real `SceneDoc`s armed and drawn through the
//!   same path a node runs, covering everything built on those primitives
//!   since: layout, transforms, strokes, shadows, gradients, clipping, images,
//!   animation, and text with its CPU-rasterized effects.
//!
//! The async boundary lives here rather than in `pmg-render`: mapping a buffer
//! is async, and how you wait differs by platform. Native blocks; the browser
//! awaits. Keeping that out of the renderer is what lets it compile to WASM
//! unchanged (ADR 0004).

use anyhow::{bail, Context, Result};
use pmg_render::corpus;
use pmg_render::parity::{Corpus, Figure, SIZE};
use std::path::{Path, PathBuf};

pub const USAGE: &str = "\
pmgctl render-golden <dir> [--font <path>]   Render the parity corpus natively
pmgctl golden-diff <a> <b>                   Compare two corpus directories

SPIKE S1 BARS
    >= 99.9% of pixels within 1/255
    max channel delta <= 2/255
    ZERO geometric displacement

Those are reported SEPARATELY on purpose. A glyph two pixels to the left is a
failure of the design promise; a shade one step off is a GPU backend difference
we have already accepted. One combined number would hide the one that matters.

FONTS
    pmg-text takes font BYTES and has no system font database. Text figures
    render only when a face is supplied, and are SKIPPED LOUDLY otherwise. Both
    halves of a cross-target run must be handed the SAME bytes; both print the
    face's length and hash so a mismatch is visible rather than mysterious.
";

/// Faces to try when `--font` is absent, in the order pmg-text's own tests try
/// them. Fine for the native self-consistency run, which compares a machine
/// against itself; a cross-target run should pass `--font` explicitly so both
/// sides are provably holding the same bytes.
const FONT_CANDIDATES: &[&str] = &[
    "/System/Library/Fonts/Supplemental/Arial.ttf",
    "/System/Library/Fonts/Supplemental/Times New Roman.ttf",
    "/System/Library/Fonts/Supplemental/Georgia.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
    "/usr/share/fonts/TTF/DejaVuSans.ttf",
    "C:\\Windows\\Fonts\\arial.ttf",
];

fn find_font(explicit: Option<&str>) -> Option<PathBuf> {
    if let Some(p) = explicit {
        return Some(PathBuf::from(p));
    }
    FONT_CANDIDATES
        .iter()
        .map(PathBuf::from)
        .find(|p| p.exists())
}

/// Render every figure natively and write `<dir>/<name>.png`.
pub fn render_golden(dir: &Path, font_arg: Option<&str>) -> Result<()> {
    std::fs::create_dir_all(dir)?;

    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        force_fallback_adapter: false,
        compatible_surface: None,
        apply_limit_buckets: false,
    }))
    .context("no wgpu adapter — is there a GPU?")?;

    let info = adapter.get_info();
    println!(
        "adapter   {} ({:?}, {:?})",
        info.name, info.backend, info.device_type
    );

    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("pmg-golden"),
        required_features: wgpu::Features::empty(),
        // Ask for the DOWNLEVEL limits, not the adapter's. The browser's limits
        // are lower than native's, and a shader validated against a dev
        // machine's generous limits is a shader that fails in the booth (R6).
        required_limits:
            wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits()),
        memory_hints: Default::default(),
        experimental_features: Default::default(),
        trace: Default::default(),
    }))
    .context("could not create a wgpu device")?;

    // --- The face, named out loud -----------------------------------------
    let font_path = find_font(font_arg);
    let font_bytes = match &font_path {
        Some(p) => {
            let b = std::fs::read(p).with_context(|| format!("reading {}", p.display()))?;
            println!(
                "font      {} ({} bytes, {})",
                p.display(),
                b.len(),
                pmg_scene::validate::address_of(&b)
            );
            Some(b)
        }
        None => {
            println!("font      NONE — text figures will be skipped");
            None
        }
    };

    // --- The primitives ----------------------------------------------------
    for figure in Figure::ALL {
        let rgba = render_primitive(&device, &queue, &Corpus::new(&device, &queue), *figure)?;
        write_png(
            &dir.join(format!("{}.png", figure.name())),
            &rgba,
            SIZE,
            SIZE,
        )?;
    }
    println!("\nprimitives  {} at {SIZE}x{SIZE}", Figure::ALL.len());

    // --- The templates -----------------------------------------------------
    let (w, h) = (corpus::CANVAS_W, corpus::CANVAS_H);
    let mut drawn = 0usize;
    let mut skipped: Vec<&str> = Vec::new();
    for t in corpus::all() {
        if t.needs_font && font_bytes.is_none() {
            skipped.push(t.name);
            continue;
        }
        let armed = corpus::arm(&t, font_bytes.as_deref())
            .map_err(|e| anyhow::anyhow!("arming {}: {e}", t.name))?;
        let rgba = render_template(&device, &queue, &armed, w, h)?;
        // A figure that drew NOTHING agrees perfectly with another figure that
        // drew nothing, so an empty frame would sail through every metric the
        // diff reports. Caught here, where the pixels are.
        if rgba.iter().skip(3).step_by(4).all(|&a| a == 0) {
            bail!(
                "{} rendered a completely transparent frame — it would pass \
                 every parity metric while testing nothing",
                t.name
            );
        }
        write_png(&dir.join(format!("{}.png", t.name)), &rgba, w, h)?;
        drawn += 1;
    }
    println!("templates   {drawn} at {w}x{h}");
    if !skipped.is_empty() {
        // Loudly. A corpus that silently shrank is a corpus that stopped
        // testing the thing it was grown to test.
        println!(
            "SKIPPED     {} text figure(s) for want of a font: {}",
            skipped.len(),
            skipped.join(", ")
        );
        println!("            pass --font <path-to-a-ttf> to render them");
    }
    println!(
        "\nwrote {} image(s) to {}",
        drawn + Figure::ALL.len(),
        dir.display()
    );
    Ok(())
}

/// Draw one armed corpus template the way a node draws a layer: composite in
/// linear, encode once on the way out.
fn render_template(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    armed: &corpus::Armed,
    w: u32,
    h: u32,
) -> Result<Vec<u8>> {
    let fb = pmg_render::draw::LinearFrame::new(device, w, h);
    let view = fb.view().clone();
    pmg_render::SceneRenderer::new(device, queue, &armed.inst).render(
        device,
        queue,
        &armed.inst,
        &armed.frame,
        &view,
        (w, h),
    );
    fb.encode(device, queue);
    readback(device, queue, fb.output(), w, h)
}

fn render_primitive(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    corpus: &Corpus,
    figure: Figure,
) -> Result<Vec<u8>> {
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("pmg-golden-target"),
        size: wgpu::Extent3d {
            width: SIZE,
            height: SIZE,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: pmg_render::TARGET_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = tex.create_view(&wgpu::TextureViewDescriptor::default());

    corpus.render(device, queue, figure, &view);
    readback(device, queue, &tex, SIZE, SIZE)
}

/// Copy a rendered texture back to RGBA8 bytes.
///
/// Both corpus widths are multiples of 64, so `w * 4` is already a multiple of
/// wgpu's 256-byte row alignment and there is no padding path. Asserted rather
/// than assumed — a padded readback that is silently mis-strided would show up
/// as a "geometric displacement" failure and send someone hunting in the
/// renderer for a bug that lives here.
fn readback(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    tex: &wgpu::Texture,
    w: u32,
    h: u32,
) -> Result<Vec<u8>> {
    let bytes_per_row = w * 4;
    assert_eq!(
        bytes_per_row % wgpu::COPY_BYTES_PER_ROW_ALIGNMENT,
        0,
        "corpus width {w} needs a padded readback"
    );

    let buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("pmg-golden-readback"),
        size: (bytes_per_row * h) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let mut enc = device.create_command_encoder(&Default::default());
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
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(h),
            },
        },
        wgpu::Extent3d {
            width: w,
            height: h,
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
    rx.recv()
        .context("readback channel closed")?
        .context("buffer map failed")?;

    let data = slice
        .get_mapped_range()
        .context("mapped range unavailable")?
        .to_vec();
    buf.unmap();
    Ok(data)
}

/// Write RGBA8 as a PNG. Shared with `pmgctl take`.
pub fn write_png_rgba(path: &Path, rgba: &[u8], w: u32, h: u32) -> Result<()> {
    write_png(path, rgba, w, h)
}

fn write_png(path: &Path, rgba: &[u8], w: u32, h: u32) -> Result<()> {
    let file = std::fs::File::create(path)?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(rgba)?;
    Ok(())
}

/// Load a corpus image, preferring raw `.rgba` over `.png`.
///
/// The browser posts back raw bytes rather than an encoded image, because
/// encoding through a canvas would put Chrome's colour management inside the
/// measurement. So the wasm side has no PNG at all, and this reads whichever
/// form is present.
///
/// Raw files carry their size in the name — `<figure>.<w>x<h>.rgba` — because
/// raw bytes do not. The older square-only form is still read: the three S1
/// primitives were posted before the corpus had a second shape.
fn read_figure(dir: &Path, name: &str) -> Result<Option<(Vec<u8>, u32, u32)>> {
    // <name>.<w>x<h>.rgba
    let prefix = format!("{name}.");
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let f = e.file_name();
            let Some(f) = f.to_str() else { continue };
            let Some(dims) = f
                .strip_prefix(&prefix)
                .and_then(|rest| rest.strip_suffix(".rgba"))
            else {
                continue;
            };
            let Some((w, h)) = dims.split_once('x') else {
                continue;
            };
            let (Ok(w), Ok(h)) = (w.parse::<u32>(), h.parse::<u32>()) else {
                continue;
            };
            let bytes = std::fs::read(e.path())?;
            if bytes.len() != (w * h * 4) as usize {
                bail!(
                    "{} says {w}x{h} but holds {} bytes",
                    e.path().display(),
                    bytes.len()
                );
            }
            return Ok(Some((bytes, w, h)));
        }
    }

    let raw = dir.join(format!("{name}.rgba"));
    if raw.exists() {
        let bytes = std::fs::read(&raw)?;
        let px = bytes.len() / 4;
        let side = (px as f64).sqrt() as u32;
        if (side * side * 4) as usize != bytes.len() {
            bail!(
                "{} is not a square RGBA image ({} bytes) — name it \
                 <figure>.<w>x<h>.rgba",
                raw.display(),
                bytes.len()
            );
        }
        return Ok(Some((bytes, side, side)));
    }
    let png_path = dir.join(format!("{name}.png"));
    if png_path.exists() {
        return Ok(Some(read_png(&png_path)?));
    }
    Ok(None)
}

/// Every figure name present in a directory, in either form.
fn figures_in(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for e in entries.flatten() {
        let f = e.file_name();
        let Some(f) = f.to_str() else { continue };
        let name = if let Some(stem) = f.strip_suffix(".png") {
            stem.to_string()
        } else if let Some(stem) = f.strip_suffix(".rgba") {
            // Drop a trailing `.<w>x<h>` if present.
            match stem.rsplit_once('.') {
                Some((head, dims)) if dims.contains('x') => head.to_string(),
                _ => stem.to_string(),
            }
        } else {
            continue;
        };
        out.push(name);
    }
    out.sort();
    out.dedup();
    out
}

fn read_png(path: &Path) -> Result<(Vec<u8>, u32, u32)> {
    let dec = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path)?));
    let mut reader = dec.read_info()?;
    let mut buf = vec![0; reader.output_buffer_size().unwrap_or(0)];
    let info = reader.next_frame(&mut buf)?;
    buf.truncate(info.buffer_size());
    if info.color_type != png::ColorType::Rgba {
        bail!("{} is not RGBA", path.display());
    }
    Ok((buf, info.width, info.height))
}

// ---------------------------------------------------------------------------
// The diff. Two INDEPENDENT metrics, reported separately.
// ---------------------------------------------------------------------------

struct ChannelStats {
    total: usize,
    within_1: usize,
    max_delta: u8,
    mean_delta: f64,
}

fn channel_stats(a: &[u8], b: &[u8]) -> ChannelStats {
    let mut within_1 = 0usize;
    let mut max_delta = 0u8;
    let mut sum = 0f64;
    let px = a.len() / 4;
    for i in 0..px {
        let mut worst = 0u8;
        for c in 0..4 {
            let d = a[i * 4 + c].abs_diff(b[i * 4 + c]);
            worst = worst.max(d);
        }
        if worst <= 1 {
            within_1 += 1;
        }
        max_delta = max_delta.max(worst);
        sum += worst as f64;
    }
    ChannelStats {
        total: px,
        within_1,
        max_delta,
        mean_delta: sum / px.max(1) as f64,
    }
}

/// Sub-pixel centroid of the alpha channel.
///
/// This is the geometric metric, and it is deliberately not a pixel count. A
/// shape rendered one pixel to the left has near-identical per-pixel deltas at
/// its edges but a centroid displaced by exactly one — which is the failure that
/// matters and the one a channel-delta number hides.
fn alpha_centroid(rgba: &[u8], w: u32) -> Option<(f64, f64)> {
    let (mut sx, mut sy, mut sw) = (0f64, 0f64, 0f64);
    for (i, px) in rgba.chunks_exact(4).enumerate() {
        let a = px[3] as f64;
        if a == 0.0 {
            continue;
        }
        let x = (i as u32 % w) as f64;
        let y = (i as u32 / w) as f64;
        sx += x * a;
        sy += y * a;
        sw += a;
    }
    (sw > 0.0).then(|| (sx / sw, sy / sw))
}

/// Total alpha mass, as a second geometric check.
///
/// Two shapes can share a centroid and differ in size. Mass catches a coverage
/// difference that is symmetric about the centre — which is exactly what a
/// different antialiasing rule would produce.
fn alpha_mass(rgba: &[u8]) -> f64 {
    rgba.chunks_exact(4).map(|p| p[3] as f64).sum::<f64>() / 255.0
}

pub fn golden_diff(a_dir: &Path, b_dir: &Path) -> Result<()> {
    println!("native : {}", a_dir.display());
    println!("wasm   : {}\n", b_dir.display());

    let mut failures: Vec<String> = Vec::new();
    let mut compared = 0;
    let mut skipped: Vec<String> = Vec::new();
    let mut worst: Vec<(f64, String)> = Vec::new();

    // Whatever is present on either side, not a fixed list: the corpus grows
    // with the renderer, and a diff that iterated a hard-coded set would go on
    // reporting PASS over figures it had stopped looking at.
    let mut names = figures_in(a_dir);
    for n in figures_in(b_dir) {
        if !names.contains(&n) {
            names.push(n);
        }
    }
    names.sort();

    let quiet = names.len() > 12;
    if quiet {
        println!(
            "{} figure(s) — per-figure detail suppressed; failures and the",
            names.len()
        );
        println!("five noisiest are printed below.\n");
    }

    for name in &names {
        let name = name.as_str();
        let (Some((a, aw, ah)), Some((b, bw, bh))) =
            (read_figure(a_dir, name)?, read_figure(b_dir, name)?)
        else {
            skipped.push(name.to_string());
            continue;
        };
        compared += 1;
        if (aw, ah) != (bw, bh) {
            failures.push(format!("{name}: size {aw}x{ah} vs {bw}x{bh}"));
            continue;
        }

        let cs = channel_stats(&a, &b);
        let pct = 100.0 * cs.within_1 as f64 / cs.total as f64;

        let ca = alpha_centroid(&a, aw);
        let cb = alpha_centroid(&b, bw);
        let disp = match (ca, cb) {
            (Some(p), Some(q)) => ((p.0 - q.0).powi(2) + (p.1 - q.1).powi(2)).sqrt(),
            _ => 0.0,
        };
        let (ma, mb) = (alpha_mass(&a), alpha_mass(&b));
        let mass_pct = if ma > 0.0 {
            100.0 * (mb - ma).abs() / ma
        } else {
            0.0
        };

        let detail = format!(
            "{name}\n  within 1/255       {pct:.4}%   (bar >= 99.9%)\n  max channel delta  \
             {}/255      (bar <= 2)\n  mean channel delta {:.4}/255\n  centroid shift     \
             {disp:.6} px  (bar = 0)\n  alpha mass delta   {mass_pct:.4}%\n",
            cs.max_delta, cs.mean_delta
        );
        if quiet {
            worst.push((cs.mean_delta.max(disp), detail));
        } else {
            println!("{detail}");
        }

        if pct < 99.9 {
            failures.push(format!("{name}: only {pct:.4}% within 1/255"));
        }
        if cs.max_delta > 2 {
            failures.push(format!("{name}: max channel delta {}/255", cs.max_delta));
        }
        // Sub-hundredth-of-a-pixel is float noise in the centroid sum, not a
        // real displacement. Anything above it means geometry moved.
        if disp > 0.01 {
            failures.push(format!("{name}: centroid displaced {disp:.4} px"));
        }
    }

    if quiet {
        worst.sort_by(|x, y| y.0.total_cmp(&x.0));
        for (_, d) in worst.iter().take(5) {
            println!("{d}");
        }
    }
    if !skipped.is_empty() {
        // Named, not counted. "3 skipped" reads as housekeeping; the names are
        // how you notice the text half of the corpus never rendered.
        println!(
            "MISSING ON ONE SIDE ({}): {}\n",
            skipped.len(),
            skipped.join(", ")
        );
    }
    println!("compared {compared} figure(s)\n");

    if compared == 0 {
        bail!("nothing to compare — run `pmgctl render-golden` and the browser harness first");
    }

    println!("--- S1 result ---");
    if failures.is_empty() {
        println!("PASS — the two corpora agree within the bars.");
        println!();
        println!("Establishes: identical coverage from our own antialiasing, and identical");
        println!("texture sampling, between the two builds of the same renderer.");
        println!();
        println!("CHECK WHAT BACKEND THE BROWSER USED before quoting this as cross-backend.");
        println!("If the browser's adapter reports the same GPU family as the native one");
        println!("(e.g. both Metal on an Apple machine), this measured two WGSL frontends");
        println!("over ONE driver — a real result, but not the cross-backend claim. D3D12");
        println!("and Vulkan need their own run. See docs/spikes/S1-render-parity.md.");
    } else {
        println!("FAIL");
        for f in &failures {
            println!("  {f}");
        }
        println!();
        println!("A CHANNEL failure is a GPU backend difference — restate ADR 0001's promise");
        println!("with the measured bound and carry on. A CENTROID failure is different and");
        println!("more serious: geometry moved, which is the thing shared code was supposed");
        println!("to guarantee. Bisect layout, then shaping, then rasterization.");
    }
    Ok(())
}
