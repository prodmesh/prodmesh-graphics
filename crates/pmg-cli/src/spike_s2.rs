//! `pmgctl spike s2` — the text typography spike.
//!
//! Brief: `docs/spikes/S2-text-typography.md`. Risk: R3.
//!
//! # Why this measures rather than looks
//!
//! The brief says "inspect for shudder at 400% zoom and in motion". Eyes are the
//! final authority, but they are a poor regression test and a worse record. Two
//! of the three hazards turn out to be measurable exactly:
//!
//! - **Shudder** is *centroid non-linearity*. Slide text by a known sub-pixel
//!   amount and the ink's centre of mass should move by exactly that amount. If
//!   glyphs snap to the pixel grid, the centroid advances in a staircase and the
//!   deviation from a straight line is the shudder, in pixels.
//! - **Fringing** is *disagreement with reference compositing*. Render text over
//!   a background on the GPU, compute the same composite on the CPU from the
//!   coverage atlas, and diff. A double-premultiply or a wrong-space blend shows
//!   up as a numeric deviation, and it shows up differently over a bright and a
//!   dark background — which is exactly why the pattern uses both.

use anyhow::{Context, Result};
use pmg_render::scene::SceneInstance;
use pmg_render::SceneRenderer;
use pmg_scene::doc::*;
use pmg_text::{FontDb, TextEngine};

pub const USAGE: &str = "\
pmgctl spike s2 --font <file.ttf> [--font2 <file.ttf> ...]

Measures the three hazards in risk R3:
  A  shudder      centroid linearity under sub-pixel animation
  B  phases       whether the 1/8-px rasterization path is exercised
  C  fringing     GPU composite vs CPU reference, over bright AND dark
  D  tnum         tabular figures hold a constant advance
  E  coverage     CJK / Arabic / emoji produce glyphs, not .notdef
  F  ticker cost  glyph-cache behaviour at full-screen-ticker scale
";

// The glyph bridge is `pmg_render::text_source::TextEngineGlyphs` — this spike
// had its own copy, which is the fourth one that module exists to prevent. A
// copy here is worse than elsewhere: S2 measures sub-pixel motion, so a spike
// that quantises differently from the renderer would be measuring itself.
type Glyphs<'a> = pmg_render::text_source::TextEngineGlyphs<'a>;

/// A minimal document: one text node, optionally over a background box.
fn text_doc(text_size: f32, canvas: (u32, u32), bg: Option<[f32; 4]>) -> SceneDoc {
    let text = Node {
        id: "t".into(),
        display_name: None,
        kind: NodeKind::Text,
        layout: Layout {
            width: Dimension::Content,
            height: Dimension::Content,
            ..Default::default()
        },
        transform: Transform2D::default(),
        style: Style {
            fill: Some(Rgba {
                rgba: [1.0, 1.0, 1.0, 1.0],
                token: None,
            }),
            ..Default::default()
        },
        clip: false,
        visible_if: None,
        content: Some(Content::Text {
            binding: Binding {
                expr: "${s}".into(),
                transform: vec![],
            },
            font: FontSpec {
                family: "F".into(),
                weight: 400,
                size: text_size,
                features: vec!["kern".into(), "liga".into()],
                tracking: 0.0,
                line_height: 1.2,
                token: None,
                style: None,
            },
            valign: Align::default(),
            align: Align::Start,
            max_lines: Some(1),
        }),
        children: vec![],
        unknown: Default::default(),
    };

    let root = Node {
        id: "root".into(),
        display_name: None,
        kind: NodeKind::Box,
        layout: Layout {
            width: Dimension::Px(canvas.0 as f32),
            height: Dimension::Px(canvas.1 as f32),
            justify: Justify::Center,
            align: Align::Center,
            ..Default::default()
        },
        transform: Transform2D::default(),
        style: Style {
            fill: bg.map(Rgba::flat),
            ..Default::default()
        },
        clip: false,
        visible_if: None,
        content: None,
        children: vec![text],
        unknown: Default::default(),
    };

    SceneDoc {
        format: "pmg.scene".into(),
        format_version: FormatVersion { major: 1, minor: 0 },
        min_reader: 1,
        id: "s2".into(),
        name: "S2".into(),
        canvas: Canvas {
            width: canvas.0,
            height: canvas.1,
            safe: None,
        },
        fields: vec![Field {
            key: "s".into(),
            ty: FieldType::Text,
            label: "s".into(),
            required: false,
            max_len: None,
            sample: None,
        }],
        fonts: vec![FontRef {
            family: "F".into(),
            weight: 400,
            style: None,
            asset: None,
            system_fallback: false,
        }],
        assets: vec![],
        root,
        timelines: vec![],
        theme: None,
        ext: Default::default(),
    }
}

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
}

fn gpu() -> Result<Gpu> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        force_fallback_adapter: false,
        compatible_surface: None,
        apply_limit_buckets: false,
    }))
    .context("no adapter")?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("s2"),
        required_features: wgpu::Features::empty(),
        required_limits:
            wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits()),
        memory_hints: Default::default(),
        experimental_features: Default::default(),
        trace: Default::default(),
    }))
    .context("no device")?;
    println!("adapter: {}\n", adapter.get_info().name);
    Ok(Gpu { device, queue })
}

/// Render one instance with a given x offset applied to the text node.
fn render_at(
    gpu: &Gpu,
    inst: &SceneInstance,
    dx: f32,
    w: u32,
    h: u32,
    renderer: &SceneRenderer,
) -> Result<Vec<u8>> {
    let mut values = std::collections::BTreeMap::new();
    values.insert("t/transform.translate.x".to_string(), dx);
    let frame = pmg_render::scene::apply_tracks(inst, &values);

    // S2 measures sub-pixel motion, so it must go through the SAME pipeline the
    // node does — including the linear composite and the encode. Measuring a
    // different path would measure a different renderer.
    let fb = pmg_render::draw::LinearFrame::new(&gpu.device, w, h);
    renderer.render(&gpu.device, &gpu.queue, inst, &frame, fb.view(), (w, h));
    fb.encode(&gpu.device, &gpu.queue);

    let bpr = w * 4;
    let buf = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: (bpr * h) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = gpu.device.create_command_encoder(&Default::default());
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
                bytes_per_row: Some(bpr),
                rows_per_image: Some(h),
            },
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    gpu.queue.submit(Some(enc.finish()));

    let slice = buf.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    gpu.device.poll(wgpu::PollType::wait_indefinitely()).ok();
    rx.recv()??;
    let out = slice.get_mapped_range().context("map")?.to_vec();
    buf.unmap();
    Ok(out)
}

/// Horizontal centre of mass of the alpha channel, in pixels.
fn centroid_x(rgba: &[u8], w: u32) -> Option<f64> {
    let (mut sx, mut sw) = (0f64, 0f64);
    for (i, p) in rgba.chunks_exact(4).enumerate() {
        let a = p[3] as f64;
        if a > 0.0 {
            sx += (i as u32 % w) as f64 * a;
            sw += a;
        }
    }
    (sw > 0.0).then(|| sx / sw)
}

pub fn main(args: &[String]) -> Result<()> {
    let font_path = args
        .iter()
        .position(|a| a == "--font")
        .and_then(|i| args.get(i + 1))
        .context("--font is required")?;
    let bytes = std::fs::read(font_path).with_context(|| format!("reading {font_path}"))?;
    println!("S2 — text typography spike");
    println!("font: {font_path} ({} bytes)\n", bytes.len());

    let gpu = gpu()?;

    shudder(&gpu, &bytes)?;
    phases_exercised(&bytes)?;
    fringing(&gpu, &bytes)?;
    tabular_figures(&bytes)?;
    script_coverage(&bytes)?;
    ticker_cost(&bytes)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// A — shudder, as centroid linearity
// ---------------------------------------------------------------------------

/// A shudder measurement: fitted slope, worst residual in pixels, and the raw
/// (dx, centroid) observations behind them. The raw samples are kept because
/// the *shape* of the error is the finding — see S2 §A.
type Shudder = (f64, f64, Vec<(f64, f64)>);

/// Slide something and report (slope, worst residual) of its centroid.
fn slide(
    gpu: &Gpu,
    inst: &SceneInstance,
    renderer: &SceneRenderer,
    w: u32,
    h: u32,
    steps: usize,
    span: f64,
) -> Result<Shudder> {
    let mut obs = Vec::new();
    for i in 0..steps {
        let dx = (i as f64 / (steps - 1) as f64) * span;
        let rgba = render_at(gpu, inst, dx as f32, w, h, renderer)?;
        let c = centroid_x(&rgba, w).context("no ink")?;
        obs.push((dx, c));
    }
    let n = obs.len() as f64;
    let (sx, sy): (f64, f64) = obs.iter().fold((0.0, 0.0), |a, (x, y)| (a.0 + x, a.1 + y));
    let (mx, my) = (sx / n, sy / n);
    let num: f64 = obs.iter().map(|(x, y)| (x - mx) * (y - my)).sum();
    let den: f64 = obs.iter().map(|(x, _)| (x - mx).powi(2)).sum();
    let slope = num / den;
    let worst = obs
        .iter()
        .map(|(x, y)| (y - (my + slope * (x - mx))).abs())
        .fold(0.0f64, f64::max);
    Ok((slope, worst, obs))
}

/// A single antialiased box — analytic SDF, NO texture sampling.
///
/// The control. If this slides smoothly and text does not, the problem is
/// atlas sampling. If both stutter, it is the geometry or transform path.
fn box_doc(canvas: (u32, u32)) -> SceneDoc {
    let mut d = text_doc(46.0, canvas, None);
    d.root.children[0] = Node {
        id: "t".into(),
        display_name: None,
        kind: NodeKind::Box,
        layout: Layout {
            width: Dimension::Px(120.0),
            height: Dimension::Px(40.0),
            ..Default::default()
        },
        transform: Transform2D::default(),
        style: Style {
            fill: Some(Rgba {
                rgba: [1.0, 1.0, 1.0, 1.0],
                token: None,
            }),
            radius: 6.0,
            ..Default::default()
        },
        clip: false,
        visible_if: None,
        content: None,
        children: vec![],
        unknown: Default::default(),
    };
    d
}

fn shudder(gpu: &Gpu, font: &[u8]) -> Result<()> {
    println!("== A. SHUDDER =============================================");
    println!("Slide text by known sub-pixel amounts; the ink centroid should");
    println!("move by exactly the same amount. Snapping shows as a staircase.\n");

    let (w, h) = (512u32, 96u32);
    let doc = text_doc(46.0, (w, h), None);
    let data: pmg_scene::bind::Data = [("s".to_string(), "Pastor Dan".to_string())]
        .into_iter()
        .collect();

    let mut db = FontDb::new();
    db.add_face(font.to_vec(), "F", 400)?;
    let measurer = pmg_text::Measurer::new(TextEngine::new(db));
    let solved = pmg_layout::solve(&doc, &data, &measurer)?;
    let mut eng = measurer.into_engine();
    let inst = {
        let mut g = Glyphs::new(&mut eng);
        SceneInstance::build(&doc, &solved, &mut g)
    };
    let renderer = SceneRenderer::new(&gpu.device, &gpu.queue, &inst);

    const STEPS: usize = 32;
    const SPAN: f64 = 2.0;
    let (slope, worst, _) = slide(gpu, &inst, &renderer, w, h, STEPS, SPAN)?;

    // THE CONTROL: an antialiased box, analytic SDF, no texture sampling.
    let bdoc = box_doc((w, h));
    let bdata: pmg_scene::bind::Data = Default::default();
    let mut bdb = FontDb::new();
    bdb.add_face(font.to_vec(), "F", 400)?;
    let bm = pmg_text::Measurer::new(TextEngine::new(bdb));
    let bsolved = pmg_layout::solve(&bdoc, &bdata, &bm)?;
    let mut beng = bm.into_engine();
    let binst = {
        let mut g = Glyphs::new(&mut beng);
        SceneInstance::build(&bdoc, &bsolved, &mut g)
    };
    let brend = SceneRenderer::new(&gpu.device, &gpu.queue, &binst);
    let (bslope, bworst, bobs) = slide(gpu, &binst, &brend, w, h, STEPS, SPAN)?;
    println!("  box control, raw:");
    println!("  {:>6} {:>10} {:>8}", "dx", "centroid", "delta");
    let mut prev: Option<f64> = None;
    for (x, y) in &bobs {
        println!(
            "  {x:6.3} {y:10.4} {:8.3}",
            prev.map(|p| y - p).unwrap_or(0.0)
        );
        prev = Some(*y);
    }
    println!();

    println!("  {:<28} {:>8}  {:>10}", "", "slope", "worst dev");
    println!(
        "  {:<28} {bslope:>8.4}  {bworst:>9.4} px",
        "antialiased box (control)"
    );
    println!(
        "  {:<28} {slope:>8.4}  {worst:>9.4} px",
        "text via glyph atlas"
    );
    println!();
    println!("  The box is analytic AA with no texture sampling. If it slides");
    println!("  smoothly and text does not, the fault is in atlas sampling — not");
    println!("  in the transform or geometry path.");
    println!();
    // A pixel-snapping renderer produces a staircase whose worst deviation is
    // ~0.5 px. Anything under a tenth of a pixel is smooth to the eye at any
    // realistic slide speed.
    if worst < 0.1 && (slope - 1.0).abs() < 0.05 {
        println!("  PASS — motion is proportional. No snapping.");
    } else if worst < 0.25 {
        println!("  MARGINAL — some non-linearity; inspect in motion.");
    } else {
        println!("  FAIL — {worst:.3} px of stair-stepping. This WILL shudder.");
    }
    println!();
    Ok(())
}

// ---------------------------------------------------------------------------
// B — is the phase path actually doing the work?
// ---------------------------------------------------------------------------

fn phases_exercised(font: &[u8]) -> Result<()> {
    println!("== B. WHERE SMOOTHNESS ACTUALLY COMES FROM ================");

    let mut db = FontDb::new();
    let face = db.add_face(font.to_vec(), "F", 400)?;
    let mut eng = TextEngine::new(db);
    let run = eng
        .shape(face, "Pastor Dan Whitfield", 46.0, &["kern".into()], 0.0)
        .context("shape")?;

    let mut used = std::collections::BTreeSet::new();
    for g in &run.glyphs {
        used.insert(pmg_text::quantize(g.x).1);
    }
    println!(
        "  distinct phases in one run   {} of {}",
        used.len(),
        pmg_text::SUBPIXEL_PHASES
    );
    println!("  phases used                  {used:?}");
    println!();
    println!("  Two mechanisms are in play and it matters which is which:");
    println!();
    println!("  1. WITHIN a run, glyphs land on fractional advances, and each is");
    println!("     rasterized AT its phase. That is what the atlas cache is for.");
    println!("  2. WHEN A RUN MOVES, the quads move and the atlas is sampled");
    println!("     bilinearly. Motion is smooth because of the SAMPLER, not");
    println!("     because glyphs are re-rasterized per frame.");
    println!();
    println!("  So test A above measures mechanism 2. Mechanism 1 is what stops");
    println!("  a static run looking unevenly spaced. Both matter; they are not");
    println!("  the same thing, and R3 as written conflates them.");
    println!();
    Ok(())
}

// ---------------------------------------------------------------------------
// C — fringing, against a CPU reference composite
// ---------------------------------------------------------------------------

fn fringing(gpu: &Gpu, font: &[u8]) -> Result<()> {
    println!("== C. FRINGING OVER A KEY =================================");
    println!("Composite white text over a BRIGHT and a DARK background on the");
    println!("GPU, and compare against the same blend computed on the CPU.");
    println!("A premultiply error deviates differently over each — which is");
    println!("exactly why the check uses both.\n");

    let (w, h) = (512u32, 96u32);
    let data: pmg_scene::bind::Data = [("s".to_string(), "Ogo".to_string())].into_iter().collect();

    for (label, bg) in [
        ("dark  (0.05)", [0.05, 0.05, 0.05, 1.0f32]),
        ("bright (0.90)", [0.90, 0.90, 0.90, 1.0f32]),
    ] {
        // Text alone, premultiplied, over nothing.
        let doc_t = text_doc(46.0, (w, h), None);
        let mut db = FontDb::new();
        db.add_face(font.to_vec(), "F", 400)?;
        let m = pmg_text::Measurer::new(TextEngine::new(db));
        let solved = pmg_layout::solve(&doc_t, &data, &m)?;
        let mut eng = m.into_engine();
        let inst_t = {
            let mut g = Glyphs::new(&mut eng);
            SceneInstance::build(&doc_t, &solved, &mut g)
        };
        let r_t = SceneRenderer::new(&gpu.device, &gpu.queue, &inst_t);
        let text_only = render_at(gpu, &inst_t, 0.0, w, h, &r_t)?;

        // Text over the background, composited by the GPU.
        let doc_b = text_doc(46.0, (w, h), Some(bg));
        let mut db2 = FontDb::new();
        db2.add_face(font.to_vec(), "F", 400)?;
        let m2 = pmg_text::Measurer::new(TextEngine::new(db2));
        let solved2 = pmg_layout::solve(&doc_b, &data, &m2)?;
        let mut eng2 = m2.into_engine();
        let inst_b = {
            let mut g = Glyphs::new(&mut eng2);
            SceneInstance::build(&doc_b, &solved2, &mut g)
        };
        let r_b = SceneRenderer::new(&gpu.device, &gpu.queue, &inst_b);
        let composited = render_at(gpu, &inst_b, 0.0, w, h, &r_b)?;

        // CPU reference: premultiplied source-over of text_only onto a flat bg.
        // src + dst*(1-a), with the background already opaque.
        let bg8 = [
            (bg[0] * 255.0).round() as i32,
            (bg[1] * 255.0).round() as i32,
            (bg[2] * 255.0).round() as i32,
        ];
        let mut worst = 0i32;
        let mut worst_at = (0u32, 0u32);
        let mut edge_px = 0usize;
        for i in 0..(w * h) as usize {
            let a = text_only[i * 4 + 3] as i32;
            if a == 0 || a == 255 {
                continue; // only the antialiased edge is interesting
            }
            edge_px += 1;
            for c in 0..3 {
                let src = text_only[i * 4 + c] as i32;
                let expect = src + bg8[c] * (255 - a) / 255;
                let got = composited[i * 4 + c] as i32;
                let d = (got - expect).abs();
                if d > worst {
                    worst = d;
                    worst_at = (i as u32 % w, i as u32 / w);
                }
            }
        }
        println!("  over {label}");
        println!("    antialiased edge pixels   {edge_px}");
        println!("    worst deviation           {worst}/255 at {worst_at:?}");
    }
    println!();
    println!("  A deviation of a couple of code values is rounding — the GPU");
    println!("  blends in 8-bit and the reference rounds once. A systematic");
    println!("  double-digit deviation, especially asymmetric between bright");
    println!("  and dark, would be a premultiply error.");
    println!();
    Ok(())
}

// ---------------------------------------------------------------------------
// D — tabular figures
// ---------------------------------------------------------------------------

fn tabular_figures(font: &[u8]) -> Result<()> {
    println!("== D. TABULAR FIGURES (tnum) ==============================");
    println!("Clocks and scores jitter if digits have different advances.\n");

    let mut db = FontDb::new();
    let face = db.add_face(font.to_vec(), "F", 400)?;
    let mut eng = TextEngine::new(db);

    for feats in [vec![], vec!["tnum".to_string()]] {
        let mut widths = Vec::new();
        for d in '0'..='9' {
            let r = eng
                .shape(face, &d.to_string(), 46.0, &feats, 0.0)
                .context("shape")?;
            widths.push(r.width);
        }
        let min = widths.iter().cloned().fold(f32::INFINITY, f32::min);
        let max = widths.iter().cloned().fold(0.0f32, f32::max);
        let label = if feats.is_empty() {
            "without tnum"
        } else {
            "with tnum   "
        };
        println!(
            "  {label}  min {min:.3}  max {max:.3}  spread {:.3} px",
            max - min
        );
    }
    println!();
    println!("  A spread of zero means digits are already tabular in this face.");
    println!("  A non-zero spread WITH tnum requested means the font does not");
    println!("  carry the feature — which is a template-authoring problem, and");
    println!("  the editor should say so rather than let a clock jitter on air.");
    println!();
    Ok(())
}

// ---------------------------------------------------------------------------
// E — script coverage
// ---------------------------------------------------------------------------

fn script_coverage(font: &[u8]) -> Result<()> {
    println!("== E. SCRIPT COVERAGE =====================================");

    let mut db = FontDb::new();
    let face = db.add_face(font.to_vec(), "F", 400)?;
    let mut eng = TextEngine::new(db);

    for (name, s) in [
        ("Latin", "Pastor Dan"),
        ("Latin + kerning", "AVATAR Two"),
        ("CJK", "日本語のテキスト"),
        ("Arabic", "مرحبا بالعالم"),
        ("Hebrew", "שלום עולם"),
        ("Combining", "é ñ ô ü  a\u{0301}"),
        ("Emoji", "🎬🎥"),
    ] {
        let r = eng.shape(face, s, 40.0, &["kern".into(), "liga".into()], 0.0);
        match r {
            Some(run) => {
                let notdef = run.glyphs.iter().filter(|g| g.id == 0).count();
                let status = if notdef == 0 {
                    "ok".to_string()
                } else {
                    format!("{notdef} .notdef")
                };
                println!(
                    "  {name:<16} {:>3} glyphs  {:>8.1} px  {status}",
                    run.glyphs.len(),
                    run.width
                );
            }
            None => println!("  {name:<16} SHAPING FAILED"),
        }
    }
    println!();
    println!("  .notdef means this face lacks the script — expected for a Latin");
    println!("  font, and the reason fonts are per-template assets. Font FALLBACK");
    println!("  is not implemented: a template must carry a face covering its");
    println!("  text, and validation should enforce that rather than silently");
    println!("  rendering tofu. That is an M5 gap, recorded here.");
    println!();
    Ok(())
}

// ---------------------------------------------------------------------------
// F — glyph cache at ticker scale
// ---------------------------------------------------------------------------

fn ticker_cost(font: &[u8]) -> Result<()> {
    println!("== F. GLYPH CACHE AT TICKER SCALE =========================");

    let mut db = FontDb::new();
    let face = db.add_face(font.to_vec(), "F", 400)?;
    let mut eng = TextEngine::new(db);

    // A full-screen ticker at 1080p is roughly 120 characters at 40 px.
    let ticker: String =
        std::iter::repeat_n("The quick brown fox jumps over the lazy dog. ", 4).collect();

    let t0 = std::time::Instant::now();
    let run = eng
        .shape(face, &ticker, 40.0, &["kern".into()], 0.0)
        .context("shape")?;
    let shape_ms = t0.elapsed().as_secs_f64() * 1000.0;

    // Cold: every glyph at every phase, which is the worst case a ticker
    // sliding continuously would eventually reach.
    let t1 = std::time::Instant::now();
    for g in &run.glyphs {
        for phase in 0..pmg_text::SUBPIXEL_PHASES as u8 {
            eng.glyph(face, g.id, 40.0, phase);
        }
    }
    let cold_ms = t1.elapsed().as_secs_f64() * 1000.0;

    let t2 = std::time::Instant::now();
    for g in &run.glyphs {
        for phase in 0..pmg_text::SUBPIXEL_PHASES as u8 {
            eng.glyph(face, g.id, 40.0, phase);
        }
    }
    let warm_ms = t2.elapsed().as_secs_f64() * 1000.0;

    println!(
        "  ticker length         {} chars, {} glyphs",
        ticker.chars().count(),
        run.glyphs.len()
    );
    println!("  shaping               {shape_ms:.3} ms");
    println!("  rasterize cold        {cold_ms:.3} ms  (all glyphs x all phases)");
    println!("  rasterize warm        {warm_ms:.3} ms  (cache hits)");
    println!(
        "  atlas after           {} entries, {} KB",
        eng.atlas().len(),
        eng.atlas().bytes() / 1024
    );
    println!();
    println!("  The cold number is a ONE-OFF at arm, not a per-frame cost — the");
    println!("  atlas is populated once and the run then animates by transform.");
    println!("  The warm number is what a frame actually pays if it touches the");
    println!("  cache at all, and a take does not.");
    println!();
    if warm_ms < 2.0 {
        println!("  PASS — warm cache is well inside a 2 ms budget.");
    } else {
        println!("  FAIL — warm cache costs {warm_ms:.2} ms, over the 2 ms bar.");
    }
    println!();
    Ok(())
}
