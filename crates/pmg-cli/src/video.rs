//! `pmgctl video` — read a clip the way a node will read it.
//!
//! Three commands, and the ordering is the same discipline as `pmgctl ndi`:
//! prove you can *read* the thing before trusting anything that plays it.
//!
//! - `probe` — what the container says, including the two answers that decide
//!   whether it is usable at all: is it variable-rate, and does it carry alpha.
//! - `frames` — decode a span to PNGs. Slow and completely honest; this is what
//!   you look at when a clip is on air and wrong.
//! - `pace` — run the real player against a synthetic output clock and report
//!   held and dropped frames. **This is the measurement that matters**, because
//!   it answers "can this machine feed this clip at this rate" without needing a
//!   card, a node, or a service.

use anyhow::{bail, Context, Result};
use pmg_media::video::{Decoder, VideoPlayer, VideoSpec};
use std::path::Path;

pub const USAGE: &str = "\
pmgctl video — read a clip the way a node will

    pmgctl video probe  <file>
    pmgctl video frames <file> [--start <n>] [--count <n>] [--out <dir>]
    pmgctl video pace   <file> [--fps N/D] [--seconds <n>] [--loop]

NOTES
    `pace` runs the player against a synthetic clock at the OUTPUT rate and
    reports held frames (the decoder fell behind — the output repeated a
    picture) and dropped frames (the media ran faster than the output, which is
    normal). Held frames on an idle machine mean this clip cannot be played
    here; on a busy one they mean it is competing with the render loop.
";

fn arg(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn rational(s: &str) -> Result<(u32, u32)> {
    let mut it = s.split('/');
    let n: u32 = it
        .next()
        .unwrap_or("")
        .parse()
        .context("bad fps numerator")?;
    let d: u32 = it.next().unwrap_or("1").parse().unwrap_or(1);
    Ok((n, d.max(1)))
}

pub fn main(args: &[String]) -> Result<()> {
    match args.first().map(String::as_str) {
        Some("probe") => probe(args.get(1).map(String::as_str)),
        Some("frames") => frames(args),
        Some("pace") => pace(args),
        _ => {
            print!("{USAGE}");
            Ok(())
        }
    }
}

fn open(path: Option<&str>) -> Result<(String, VideoSpec)> {
    let path = path.context("give a file")?.to_string();
    let spec = pmg_media::video::probe(&path)?;
    Ok((path, spec))
}

fn describe(spec: &VideoSpec) {
    println!("  codec       {}", spec.codec);
    println!("  size        {}x{}", spec.width, spec.height);
    println!(
        "  rate        {}/{}  ({:.3} fps)",
        spec.fps_num,
        spec.fps_den,
        spec.fps()
    );
    println!("  duration    {:.3} s", spec.duration_s);
    println!("  frames      {}", spec.frames);
    println!(
        "  alpha       {}",
        if spec.has_alpha {
            "yes — frames are premultiplied on the way in"
        } else {
            "no — this will key as a solid rectangle"
        }
    );
    println!(
        "  frame size  {:.1} MB RGBA",
        spec.frame_bytes() as f64 / 1e6
    );
}

fn probe(path: Option<&str>) -> Result<()> {
    let (path, spec) = open(path)?;
    println!("{path}");
    describe(&spec);
    Ok(())
}

fn frames(args: &[String]) -> Result<()> {
    let (path, spec) = open(args.get(1).map(String::as_str))?;
    let start: u64 = arg(args, "--start")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let count: u64 = arg(args, "--count")
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    let out = arg(args, "--out").unwrap_or_else(|| ".".into());
    std::fs::create_dir_all(&out)?;

    describe(&spec);
    let dec = Decoder::open(&path, &spec, start, false)?;
    // The player is bypassed on purpose: this decodes frame by frame, so what
    // lands on disk is what the decoder produced and not what a clock selected.
    let mut player = VideoPlayer::new(dec, (spec.fps_num, spec.fps_den), false);

    for i in 0..count {
        // Wait, deliberately — this command is not on a frame budget, and a
        // `None` here would write nothing and look like a decode failure.
        let mut frame = None;
        for _ in 0..2000 {
            if let Some(f) = player.frame_for(i) {
                if f.index == i {
                    frame = Some((f.index, f.rgba.clone()));
                    break;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let Some((index, rgba)) = frame else {
            bail!("frame {} never arrived", start + i);
        };
        let name = format!("{out}/frame-{:06}.png", start + index);
        write_png(&name, spec.width, spec.height, &rgba)?;
        println!("  wrote {name}");
    }
    Ok(())
}

/// Write premultiplied RGBA straight out.
///
/// **Not unpremultiplied first.** A PNG viewer will show these composited over
/// its own background, and a soft edge that looks right there is the check —
/// unpremultiplying on the way out would hide exactly the mistake this command
/// exists to reveal.
fn write_png(path: &str, w: u32, h: u32, rgba: &[u8]) -> Result<()> {
    let file = std::fs::File::create(Path::new(path))?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(rgba)?;
    Ok(())
}

fn pace(args: &[String]) -> Result<()> {
    let (path, spec) = open(args.get(1).map(String::as_str))?;
    let out_fps = rational(&arg(args, "--fps").unwrap_or_else(|| "60000/1001".into()))?;
    let seconds: f64 = arg(args, "--seconds")
        .and_then(|v| v.parse().ok())
        .unwrap_or(10.0);
    let looping = args.iter().any(|a| a == "--loop");

    describe(&spec);
    println!(
        "\n  output clock {}/{} ({:.3} fps), {seconds:.0} s{}",
        out_fps.0,
        out_fps.1,
        out_fps.0 as f64 / out_fps.1 as f64,
        if looping { ", looping" } else { "" }
    );

    let dec = Decoder::open(&path, &spec, 0, looping)?;
    let mut player = VideoPlayer::new(dec, out_fps, looping);

    // Pre-roll, exactly as a node does at arm. Without it the clock starts
    // ahead of the decoder and the player spends the first second catching up —
    // which reads as an underrun and is not one.
    let t_pre = std::time::Instant::now();
    if !player.preroll(std::time::Duration::from_secs(5)) {
        bail!("no frame arrived within 5 s — is this file readable?");
    }
    println!(
        "  pre-roll    {:.1} ms",
        t_pre.elapsed().as_secs_f64() * 1e3
    );

    let total = (seconds * out_fps.0 as f64 / out_fps.1 as f64).round() as u64;
    let period = std::time::Duration::from_secs_f64(out_fps.1 as f64 / out_fps.0 as f64);
    let start = std::time::Instant::now();
    let mut worst = std::time::Duration::ZERO;
    let mut delivered = 0u64;

    for n in 0..total {
        // A synthetic output clock: sleep until this frame's deadline, exactly
        // as a real output would call us. Sleeping a fixed period instead would
        // let our own overhead accumulate and turn into fake held frames.
        let deadline = start + period.mul_f64(n as f64);
        if let Some(d) = deadline.checked_duration_since(std::time::Instant::now()) {
            std::thread::sleep(d);
        }
        let t0 = std::time::Instant::now();
        if player.frame_for(n).is_some() {
            delivered += 1;
        }
        worst = worst.max(t0.elapsed());
        if player.finished() {
            println!("  clip ended at output frame {n}");
            break;
        }
    }

    println!("\n  delivered   {delivered} of {total}");
    println!(
        "  held        {}  (decoder behind — the output repeated a picture)",
        player.held()
    );
    println!(
        "  dropped     {}  (media ahead of the output — expected when faster)",
        player.dropped()
    );
    println!(
        "  worst pull  {:.3} ms against a {:.2} ms budget",
        worst.as_secs_f64() * 1e3,
        period.as_secs_f64() * 1e3
    );
    if player.held() > 0 {
        println!(
            "\n  Held frames mean this machine could not feed this clip at this rate.\n  \
             That is a decode budget problem, not a bug."
        );
    }
    Ok(())
}
