//! `pmgctl scene` — compile, inspect, and format SceneDocs.
//!
//! The native half of the compiler. The browser runs the *same* `pmg-scene`
//! code compiled to WASM, so a template compiled on a laptop and one compiled
//! here are byte-identical — which is what makes `template_rev` a usable
//! identity (ADR 0001).

use anyhow::{bail, Context, Result};
use pmg_scene::{cbor, doc::SceneDoc, fixture, validate};
use std::path::Path;

pub const USAGE: &str = "\
pmgctl scene — the SceneDoc compiler (M1)

    pmgctl scene compile <in.json> <out.pmgscene>
                                  Canonical CBOR + <out>.manifest.json
    pmgctl scene fmt <file>       Round-trip through the JSON mirror
    pmgctl scene inspect <file>   Structure, warnings, and template_rev
    pmgctl scene fixture [out]    Write the reference lower third as JSON

Accepts .json (the mirror) or .pmgscene (canonical CBOR) anywhere a file is
taken — the extension decides.
";

pub fn main(args: &[String]) -> Result<()> {
    match args.first().map(String::as_str) {
        Some("compile") => match (args.get(1), args.get(2)) {
            (Some(i), Some(o)) => compile(Path::new(i), Path::new(o)),
            _ => {
                print!("{USAGE}");
                Ok(())
            }
        },
        Some("fmt") => fmt(Path::new(
            args.get(1).map(String::as_str).unwrap_or_default(),
        )),
        Some("inspect") => inspect(Path::new(
            args.get(1).map(String::as_str).unwrap_or_default(),
        )),
        Some("fixture") => {
            let out = args
                .get(1)
                .map(String::as_str)
                .unwrap_or("lower-third.pmgscene.json");
            let doc = fixture::lower_third();
            std::fs::write(out, serde_json::to_string_pretty(&doc)?)?;
            println!("wrote {out}");
            Ok(())
        }
        _ => {
            print!("{USAGE}");
            Ok(())
        }
    }
}

/// Load either representation. The extension decides which.
fn load(path: &Path) -> Result<SceneDoc> {
    if path.as_os_str().is_empty() {
        bail!("no file given");
    }
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    if path.extension().is_some_and(|e| e == "json") {
        Ok(serde_json::from_slice(&bytes)?)
    } else {
        cbor::from_canonical(&bytes).map_err(Into::into)
    }
}

fn compile(input: &Path, output: &Path) -> Result<()> {
    let doc = load(input)?;
    let (bytes, manifest) = validate::compile(&doc)?;

    std::fs::write(output, &bytes)?;
    let manifest_path = output.with_extension("manifest.json");
    std::fs::write(&manifest_path, serde_json::to_string_pretty(&manifest)?)?;

    println!("template_rev  {}", manifest.template_rev);
    println!("scene         {} ({} bytes)", output.display(), bytes.len());
    println!("manifest      {}", manifest_path.display());
    println!();
    println!("The server validates the MANIFEST and stores the scene as an opaque blob.");
    println!("It never parses the scene format — see ADR 0004.");

    if !manifest.warnings.is_empty() {
        println!("\n{} warning(s):", manifest.warnings.len());
        for w in &manifest.warnings {
            println!("  [{}] {}", w.code, w.detail);
        }
    }
    Ok(())
}

fn fmt(path: &Path) -> Result<()> {
    let doc = load(path)?;
    // Prove the round trip rather than asserting it: re-read what we are about
    // to write and require it to hash the same.
    let json = serde_json::to_string_pretty(&doc)?;
    let back: SceneDoc = serde_json::from_str(&json)?;
    if validate::template_rev(&back)? != validate::template_rev(&doc)? {
        bail!("formatting changed the document's identity — this is a bug in the mirror");
    }
    println!("{json}");
    Ok(())
}

fn inspect(path: &Path) -> Result<()> {
    let doc = load(path)?;
    let rev = validate::template_rev(&doc)?;
    let warnings = validate::validate(&doc)?;

    println!("{}  ({})", doc.name, doc.id);
    println!("  template_rev   {rev}");
    println!(
        "  format         v{}.{} (min_reader {})",
        doc.format_version.major, doc.format_version.minor, doc.min_reader
    );
    println!(
        "  canvas         {}x{}",
        doc.canvas.width, doc.canvas.height
    );

    println!("\n  fields");
    for f in &doc.fields {
        let req = if f.required { " required" } else { "" };
        let max = f.max_len.map(|n| format!(" max {n}")).unwrap_or_default();
        println!("    {:<10} {:?}{req}{max}", f.key, f.ty);
    }

    println!("\n  tree");
    print_node(&doc.root, 4);

    println!("\n  timelines");
    for t in &doc.timelines {
        println!(
            "    {:<10} {:>6.0} ms  {} track(s)  on_complete {:?}",
            t.name,
            t.duration_ms,
            t.tracks.len(),
            t.on_complete
        );
        for tr in &t.tracks {
            // Flag the expensive kind inline, where a designer will see it.
            let flag = if tr.animates_layout() {
                "  <- RELAYOUTS EVERY FRAME"
            } else {
                ""
            };
            println!("      {}{flag}", tr.target);
        }
    }

    if warnings.is_empty() {
        println!("\n  no warnings");
    } else {
        println!("\n  {} warning(s)", warnings.len());
        for w in &warnings {
            println!("    [{}] {}", w.code, w.detail);
        }
    }
    Ok(())
}

fn print_node(n: &pmg_scene::doc::Node, indent: usize) {
    let pad = " ".repeat(indent);
    let kind = if n.kind.is_renderable() {
        format!("{:?}", n.kind).to_lowercase()
    } else {
        format!("{:?} (UNRENDERABLE)", n.kind)
    };
    let vis = n
        .visible_if
        .as_ref()
        .map(|v| format!("  visible_if {} {:?}", v.field, v.op))
        .unwrap_or_default();
    let unknown = if n.unknown.is_empty() {
        String::new()
    } else {
        format!("  +{} unknown key(s) preserved", n.unknown.len())
    };
    println!("{pad}{:<12} {kind}{vis}{unknown}", n.id);
    for c in &n.children {
        print_node(c, indent + 2);
    }
}
