//! `pmgctl take` — the M1 vertical slice, end to end.
//!
//! Compile a template, bind an Item's data, solve layout, shape and rasterize
//! text, build a `SceneInstance`, then play `In` → hold → `Out` while rendering
//! every frame and sending it over NDI with key and fill.
//!
//! This is the first point at which the demo exists.
//!
//! # The arm / take split, made real
//!
//! Everything expensive happens **once, at arm**: layout, binding evaluation,
//! shaping, rasterization, atlas packing. Per frame, only timelines are
//! evaluated and transforms applied. That is the `layout.*` versus `transform.*`
//! split from `docs/specs/scene-format.md` §3.2 — and the reason a take is cheap
//! enough to hit a frame.
//!
//! The 17.55 ms measurement in `docs/INTEGRATION-NOTES.md` is what happens when
//! you ignore it and regenerate per frame.

use anyhow::{bail, Context, Result};
use pmg_render::scene::SceneInstance;
use pmg_render::text_source::TextEngineGlyphs;
use pmg_render::{timeline, SceneRenderer};
use pmg_scene::doc::SceneDoc;
use pmg_text::{Measurer, TextEngine};
use std::path::Path;

pub const USAGE: &str = "\
pmgctl take — render an Item on air (M1)

    pmgctl take <template.json> --font <file.ttf>
                [--set k=v ...]         Item data (defaults to field samples)
                [--name <ndi name>]     Default \"ProdMesh Graphics\"
                [--fps N/D]             Default 60000/1001
                [--hold <seconds>]      Time on air between In and Out
                [--png <dir>]           Also write frames as PNGs
                [--frames-only]         Skip NDI; just render and report
                [--clip key=<file>]     Bind a clip to an asset key (repeatable)

Plays In, holds, then plays Out. Everything expensive happens once at arm;
per frame only timelines are evaluated and transforms applied.
";

fn arg(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

/// Clips bound to asset keys, standing in for what a node gets from its CAS.
///
/// Serves poster frames as an [`pmg_render::scene::ImageSource`] at arm, then
/// drives the players per frame — the same two jobs, in the same order, as
/// `pmg-node`'s runtime. Keeping them side by side here is what makes this
/// command a real test of playback rather than a separate implementation of it.
struct Clips {
    players: std::collections::BTreeMap<String, pmg_media::video::VideoPlayer>,
    posters: std::collections::BTreeMap<String, (u32, u32, std::sync::Arc<Vec<u8>>)>,
    start: Option<u64>,
}

impl Clips {
    fn open(args: &[String], fps: (u32, u32)) -> Result<Self> {
        let mut me = Self {
            players: Default::default(),
            posters: Default::default(),
            start: None,
        };
        for (i, a) in args.iter().enumerate() {
            if a != "--clip" {
                continue;
            }
            let spec_arg = args.get(i + 1).context("--clip wants key=file")?;
            let (key, file) = spec_arg
                .split_once('=')
                .context("--clip looks like key=path/to/clip.mov")?;

            let spec = pmg_media::video::probe(file)?;
            let dec = pmg_media::video::Decoder::open(file, &spec, 0, true)?;
            let mut player = pmg_media::video::VideoPlayer::new(dec, fps, true);
            if !player.preroll(std::time::Duration::from_secs(10)) {
                bail!("{file}: no frame decoded within 10 s");
            }
            let poster = player
                .frame_for(0)
                .map(|f| f.rgba.clone())
                .context("no first frame")?;
            println!(
                "clip {key}: {} {}x{} {:.3} fps, alpha {}",
                spec.codec,
                spec.width,
                spec.height,
                spec.fps(),
                spec.has_alpha
            );
            me.posters
                .insert(key.to_string(), (spec.width, spec.height, poster));
            me.players.insert(key.to_string(), player);
        }
        Ok(me)
    }

    fn advance(
        &mut self,
        queue: &wgpu::Queue,
        renderer: &SceneRenderer,
        inst: &SceneInstance,
        frame: u64,
    ) {
        if self.players.is_empty() {
            return;
        }
        if self.start.is_none() {
            self.start = Some(frame);
        }
        let n = frame - self.start.unwrap_or(frame);
        for slot in &inst.videos {
            let Some(p) = self.players.get_mut(&slot.asset_key) else {
                continue;
            };
            let (w, h) = (p.spec().width, p.spec().height);
            if let Some(f) = p.frame_for(n) {
                let _ = renderer.update_image(queue, slot.image, w, h, &f.rgba);
            }
        }
    }

    fn is_empty(&self) -> bool {
        self.players.is_empty()
    }

    fn report(&self) {
        for (key, p) in &self.players {
            println!(
                "  clip {key}     {} held, {} dropped",
                p.held(),
                p.dropped()
            );
        }
    }
}

impl pmg_render::scene::ImageSource for Clips {
    fn image(&mut self, key: &str) -> Option<(u32, u32, std::sync::Arc<Vec<u8>>)> {
        self.posters.get(key).cloned()
    }
}

pub fn main(args: &[String]) -> Result<()> {
    let template = args.first().map(String::as_str).unwrap_or_default();
    if template.is_empty() {
        print!("{USAGE}");
        return Ok(());
    }

    let doc: SceneDoc = {
        let bytes =
            std::fs::read(Path::new(template)).with_context(|| format!("reading {template}"))?;
        if template.ends_with(".json") {
            serde_json::from_slice(&bytes)?
        } else {
            pmg_scene::cbor::from_canonical(&bytes)?
        }
    };
    let warnings = pmg_scene::validate::validate(&doc)?;
    for w in &warnings {
        println!("warning [{}] {}", w.code, w.detail);
    }

    // --- Item data: field samples unless overridden -----------------------
    let mut data: pmg_scene::bind::Data = doc
        .fields
        .iter()
        .filter_map(|f| f.sample.clone().map(|s| (f.key.clone(), s)))
        .collect();
    let mut i = 1;
    while i < args.len() {
        if args[i] == "--set" {
            if let Some((k, v)) = args.get(i + 1).and_then(|kv| kv.split_once('=')) {
                data.insert(k.to_string(), v.to_string());
            }
            i += 2;
        } else {
            i += 1;
        }
    }

    // --- Fonts, as bytes --------------------------------------------------
    let font_path = arg(args, "--font").context(
        "--font is required. pmg-text only accepts font BYTES — there is no system font \
         database, because a template naming a system font would render differently on a \
         designer's laptop and a booth Mac",
    )?;
    let font_bytes = std::fs::read(&font_path).with_context(|| format!("reading {font_path}"))?;

    let mut db = pmg_text::FontDb::new();
    // Register the bytes under every family the document asks for, so a
    // developer can drive the slice with whatever font is to hand.
    for f in &doc.fonts {
        db.add_face(font_bytes.clone(), f.family.clone(), f.weight)?;
    }
    if db.is_empty() {
        db.add_face(font_bytes.clone(), "Default", 400)?;
    }

    let (fps_num, fps_den) = match arg(args, "--fps") {
        Some(s) => {
            let (a, b) = s.split_once('/').context("--fps looks like 60000/1001")?;
            (a.parse::<u32>()?, b.parse::<u32>()?)
        }
        None => (60000, 1001),
    };
    let fps = fps_num as f64 / fps_den as f64;
    let hold_s: f64 = arg(args, "--hold")
        .and_then(|s| s.parse().ok())
        .unwrap_or(3.0);

    // ======================= ARM ==========================================
    let arm_start = std::time::Instant::now();

    let measurer = Measurer::new(TextEngine::new(db));
    let solved = pmg_layout::solve(&doc, &data, &measurer)?;
    let mut engine = measurer.into_engine();

    // Clips named on the command line, each pre-rolled so its first frame is
    // ready before the clock starts. The poster frame is what sizes the
    // texture, exactly as it does on a node.
    let mut clips = Clips::open(args, (fps_num, fps_den))?;
    let inst = SceneInstance::build_with(
        &doc,
        &solved,
        &mut TextEngineGlyphs::new(&mut engine),
        &mut clips,
    );
    let arm_ms = arm_start.elapsed().as_secs_f64() * 1000.0;

    println!("armed in {arm_ms:.1} ms");
    println!("  canvas        {}x{}", inst.canvas_w, inst.canvas_h);
    println!("  nodes         {}", inst.nodes.len());
    println!(
        "  glyph atlas   {}x{} ({} glyphs cached, {} KB)",
        inst.atlas.width,
        inst.atlas.height,
        engine.atlas().len(),
        engine.atlas().bytes() / 1024
    );
    if !solved.omitted.is_empty() {
        println!("  omitted       {:?}  (visible_if)", solved.omitted);
    }
    if inst.degraded {
        println!("  DEGRADED — the document contains a node kind this reader cannot draw");
    }

    // --- The frame plan ---------------------------------------------------
    let tl_in = timeline::find(&doc, "In");
    let tl_out = timeline::find(&doc, "Out");
    let in_ms = tl_in.map(|t| t.duration_ms).unwrap_or(0.0) as f64;
    let out_ms = tl_out.map(|t| t.duration_ms).unwrap_or(0.0) as f64;
    let hold_ms = hold_s * 1000.0;
    let total_ms = in_ms + hold_ms + out_ms;
    let total_frames = (total_ms / 1000.0 * fps).round() as u64;

    println!(
        "\nplan: In {in_ms:.0} ms -> hold {hold_ms:.0} ms -> Out {out_ms:.0} ms  \
         = {total_frames} frames at {fps:.3} fps"
    );

    // ======================= RENDER =======================================
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        force_fallback_adapter: false,
        compatible_surface: None,
        apply_limit_buckets: false,
    }))
    .context("no wgpu adapter")?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("pmg-take"),
        required_features: wgpu::Features::empty(),
        required_limits:
            wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits()),
        memory_hints: Default::default(),
        experimental_features: Default::default(),
        trace: Default::default(),
    }))
    .context("no wgpu device")?;
    println!("adapter: {}", adapter.get_info().name);

    let renderer = SceneRenderer::new(&device, &queue, &inst);
    let (w, h) = (inst.canvas_w, inst.canvas_h);

    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("pmg-take-target"),
        size: wgpu::Extent3d {
            width: w,
            height: h,
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

    let bytes_per_row = w * 4;
    if bytes_per_row % wgpu::COPY_BYTES_PER_ROW_ALIGNMENT != 0 {
        bail!("canvas width {w} needs row padding, which this M1 path does not do yet");
    }
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("pmg-take-readback"),
        size: (bytes_per_row * h) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let png_dir = arg(args, "--png");
    if let Some(d) = &png_dir {
        std::fs::create_dir_all(d)?;
    }
    let frames_only = args.iter().any(|a| a == "--frames-only");

    // NDI, unless we were told to skip it.
    let ndi = if frames_only {
        None
    } else {
        Some(pmg_out_ndi::Sender::open(
            &arg(args, "--name").unwrap_or_else(|| "ProdMesh Graphics".into()),
            w,
            h,
            fps_num as i32,
            fps_den as i32,
        )?)
    };

    // With no clip and no NDI this loop runs flat out, which is the point: the
    // effective rate it reports is the render cost with nothing pacing it.
    //
    // A clip changes that. Asking a player for frames at 280 fps when its
    // decoder produces 170 makes every reading meaningless — the first version
    // of this reported 247 held frames out of 253 and none of them were real.
    // When a clip is bound the loop runs at the OUTPUT rate, because that is
    // the only rate at which "did playback keep up" is a question.
    let pace = (!clips.is_empty() && ndi.is_none())
        .then(|| std::time::Duration::from_secs_f64(fps_den as f64 / fps_num as f64));
    if pace.is_some() {
        println!("\npacing to {fps:.3} fps — a clip is bound, and playback is only measurable in real time");
    }

    println!("\nrendering…");
    let start = std::time::Instant::now();
    let mut worst_ms = 0.0f64;
    // A worst case is one number and hides its own frequency. One frame over
    // budget in ten minutes is a shrug; one in ten is a fault, and the worst
    // case reads the same either way.
    let mut over_budget = 0u32;
    let budget_ms = 1000.0 * fps_den as f64 / fps_num as f64;
    let mut phase_log = Vec::new();

    for frame in 0..total_frames {
        let t = timeline::frame_time_ms(frame, fps_num, fps_den) as f64;

        if let Some(p) = pace {
            // Sleep to this frame's DEADLINE, not for a fixed period: a fixed
            // sleep lets our own overhead accumulate into fake held frames.
            let deadline = start + p.mul_f64(frame as f64);
            if let Some(d) = deadline.checked_duration_since(std::time::Instant::now()) {
                std::thread::sleep(d);
            }
        }

        // Timed from HERE, so the per-frame cost includes uploading the clip's
        // texture. That upload is 8.3 MB at 1080p and is genuinely per frame;
        // measuring the draw without it would flatter the budget.
        let f0 = std::time::Instant::now();

        // Clips advance against the OUTPUT frame counter, never wall time —
        // the same rule the node's render loop obeys.
        clips.advance(&queue, &renderer, &inst, frame);

        // Which timeline is driving, and at what local time.
        let (tl, local, phase) = if t < in_ms {
            (tl_in, t, "in")
        } else if t < in_ms + hold_ms {
            // Hold: the In timeline's final state, held. NOT re-evaluated from
            // zero — that would restart the animation every frame.
            (tl_in, in_ms, "hold")
        } else {
            (tl_out, t - in_ms - hold_ms, "out")
        };
        if phase_log.last().map(|(p, _)| *p) != Some(phase) {
            phase_log.push((phase, frame));
        }

        let values = tl
            .map(|t| timeline::evaluate(t, local as f32))
            .unwrap_or_default();
        let frame_nodes = pmg_render::scene::apply_tracks(&inst, &values);
        renderer.render(&device, &queue, &inst, &frame_nodes, &view);

        // Read back.
        let mut enc = device.create_command_encoder(&Default::default());
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
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

        let slice = readback.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        device.poll(wgpu::PollType::wait_indefinitely()).ok();
        rx.recv()??;
        let rgba = slice.get_mapped_range().context("mapped range")?.to_vec();
        readback.unmap();

        let ms = f0.elapsed().as_secs_f64() * 1000.0;
        worst_ms = worst_ms.max(ms);
        if ms > budget_ms {
            over_budget += 1;
        }

        if let Some(s) = &ndi {
            s.send_rgba_premultiplied(&rgba)?;
        }
        if let Some(d) = &png_dir {
            // Sample frames only — 240 PNGs of a 4-second take helps nobody.
            if frame % 20 == 0 {
                crate::golden::write_png_rgba(
                    Path::new(&format!("{d}/frame-{frame:04}-{phase}.png")),
                    &rgba,
                    w,
                    h,
                )?;
            }
        }
    }

    let elapsed = start.elapsed().as_secs_f64();
    println!("\n--- result ---");
    clips.report();
    println!("  frames           {total_frames}");
    println!("  elapsed          {elapsed:.2}s");
    println!(
        "  effective rate   {:.3} fps (asked {fps:.3})",
        total_frames as f64 / elapsed
    );
    println!("  over budget      {over_budget} of {total_frames} frames");
    println!(
        "  worst frame      {worst_ms:.2} ms  (budget {:.2} ms)",
        1000.0 / fps
    );
    println!("  arm cost         {arm_ms:.1} ms, once");
    print!("  phases           ");
    for (p, f) in &phase_log {
        print!("{p}@{f} ");
    }
    println!();
    println!();
    println!("Per frame this evaluated timelines and applied transforms — no layout,");
    println!("no shaping, no rasterization. That split is why a take can hit a frame.");
    Ok(())
}
