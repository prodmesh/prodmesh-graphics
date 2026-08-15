//! `pmgctl text` — inspect shaping, metrics, and the subpixel atlas.
//!
//! Exists because the failures R3 names are invisible in a screenshot: a glyph
//! snapping between whole pixels looks fine in a still and shudders in motion.
//! Numbers catch it; eyes do not.

use anyhow::{bail, Context, Result};
use pmg_text::{FontDb, TextEngine, SUBPIXEL_PHASES};
use std::path::Path;

pub const USAGE: &str = "\
pmgctl text — shaping and rasterization (M1c)

    pmgctl text metrics <font.ttf> [--size N]
                                  Vertical metrics, and the rule behind them
    pmgctl text shape <font.ttf> <string> [--size N] [--features a,b]
                                  Shaped glyphs with fractional positions
    pmgctl text phases <font.ttf> <char> [--size N]
                                  Rasterize one glyph at every subpixel phase

Fonts are loaded as BYTES. There is no system font database in pmg-text — a
template that named a system font would render differently on a designer's
laptop and a booth Mac, silently.
";

fn arg(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn size_of(args: &[String]) -> f32 {
    arg(args, "--size")
        .and_then(|s| s.parse().ok())
        .unwrap_or(46.0)
}

fn load(path: &str) -> Result<(TextEngine, pmg_text::FaceId)> {
    if path.is_empty() {
        bail!("no font given");
    }
    let bytes = std::fs::read(Path::new(path)).with_context(|| format!("reading {path}"))?;
    let n = bytes.len();
    let mut db = FontDb::new();
    let face = db.add_face(bytes, "Loaded", 400)?;
    println!("font   {path}  ({n} bytes, loaded as bytes)");
    Ok((TextEngine::new(db), face))
}

pub fn main(args: &[String]) -> Result<()> {
    match args.first().map(String::as_str) {
        Some("metrics") => metrics(args),
        Some("shape") => shape(args),
        Some("phases") => phases(args),
        _ => {
            print!("{USAGE}");
            Ok(())
        }
    }
}

fn metrics(args: &[String]) -> Result<()> {
    let (mut eng, face) = load(args.get(1).map(String::as_str).unwrap_or_default())?;
    let size = size_of(args);
    let m = eng.metrics(face, size).context("no metrics")?;

    println!("size   {size} px\n");
    println!("  units per em        {}", m.units_per_em);
    println!("  ascent              {:>8.3} px", m.ascent);
    println!(
        "  descent             {:>8.3} px  (positive magnitude)",
        m.descent
    );
    println!("  leading             {:>8.3} px", m.leading);
    println!("  natural line height {:>8.3} px", m.natural_line_height());
    println!();
    println!("RULE: OS/2 typographic metrics when the font sets USE_TYPO_METRICS");
    println!("(fsSelection bit 7), otherwise hhea. Same rule browsers use.");
    println!();
    println!("This is chosen ONCE and never changed — changing it would move every");
    println!("existing template by a couple of pixels, silently. It cannot drift,");
    println!("because fonts are content-addressed: a given font hash always has the");
    println!("same flag and therefore the same baseline.");
    Ok(())
}

fn shape(args: &[String]) -> Result<()> {
    let (mut eng, face) = load(args.get(1).map(String::as_str).unwrap_or_default())?;
    let text = args.get(2).cloned().unwrap_or_else(|| "Pastor Dan".into());
    let size = size_of(args);
    let features: Vec<String> = arg(args, "--features")
        .map(|s| s.split(',').map(str::to_string).collect())
        .unwrap_or_else(|| vec!["kern".into(), "liga".into()]);

    let run = eng
        .shape(face, &text, size, &features, 0.0)
        .context("shaping failed")?;

    println!("size   {size} px   features {features:?}");
    println!("text   {text:?}\n");
    println!(
        "  {:<6} {:>6} {:>10} {:>8} {:>7}",
        "glyph", "id", "x", "whole", "phase"
    );
    for g in &run.glyphs {
        let (whole, phase) = pmg_text::quantize(g.x);
        println!(
            "  {:<6} {:>6} {:>10.4} {:>8} {:>7}",
            "", g.id, g.x, whole, phase
        );
    }
    println!("\n  run width           {:>8.3} px", run.width);
    println!(
        "  natural line height {:>8.3} px",
        run.metrics.natural_line_height()
    );
    println!();
    println!("Note the FRACTIONAL x positions. Rounding those to whole pixels is what");
    println!("makes animated text shudder — see `pmgctl text phases`.");
    Ok(())
}

fn phases(args: &[String]) -> Result<()> {
    let (mut eng, face) = load(args.get(1).map(String::as_str).unwrap_or_default())?;
    let ch = args.get(2).cloned().unwrap_or_else(|| "M".into());
    let size = size_of(args);

    let run = eng
        .shape(face, &ch, size, &[], 0.0)
        .context("shaping failed")?;
    let gid = run.glyphs.first().context("no glyph")?.id;

    println!("glyph {gid} ({ch:?}) at {size} px, across {SUBPIXEL_PHASES} subpixel phases\n");
    println!(
        "  {:>5} {:>7} {:>7} {:>6} {:>6} {:>12}",
        "phase", "offset", "w x h", "left", "top", "coverage sum"
    );

    let mut sums = Vec::new();
    for phase in 0..SUBPIXEL_PHASES as u8 {
        let g = eng.glyph(face, gid, size, phase).context("rasterize")?;
        let sum: u64 = g.coverage.iter().map(|c| *c as u64).sum();
        sums.push(sum);
        println!(
            "  {:>5} {:>7.3} {:>3}x{:<3} {:>6} {:>6} {:>12}",
            phase,
            phase as f32 / SUBPIXEL_PHASES as f32,
            g.width,
            g.height,
            g.left,
            g.top,
            sum
        );
    }

    let all_same = sums.windows(2).all(|w| w[0] == w[1]);
    println!();
    if all_same {
        println!("!! EVERY PHASE IS IDENTICAL. Subpixel positioning is NOT working, and");
        println!("!! animated text will shudder as glyphs snap between whole pixels.");
    } else {
        println!("Phases differ, which is what we want: each fractional offset is its own");
        println!("rasterization, cached separately. That is what stops text shuddering as");
        println!("it slides — the failure R3 names first.");
    }
    println!();
    println!("Coverage sums vary slightly because antialiasing redistributes the same ink");
    println!("across different pixel boundaries.");
    Ok(())
}
