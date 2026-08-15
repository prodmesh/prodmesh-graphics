//! `pmgctl` — the workspace's CLI and, more importantly, its selftest.
//!
//! `--selftest` is inherited from prodmesh-caption and prodmesh-rta and it
//! serves the same purpose here: one binary, no test framework, prints `PASS` or
//! a specific failure. It runs on the machine that has the problem, it needs no
//! toolchain beyond the shipped binary, and **it gates releases in CI**.
//!
//! Run it after touching `pmg-scene`, `pmg-layout`, `pmg-anim`, or `pmg-color`.

use anyhow::{bail, Result};

const USAGE: &str = "\
pmgctl — ProdMesh Graphics command line

USAGE:
    pmgctl --selftest              Run every crate's assertions. Prints PASS.
    pmgctl --version

PLANNED (see docs/ROADMAP.md):
    pmgctl scene compile <in> <out>    M1
    pmgctl scene fmt <file>            M1  canonical CBOR <-> JSON mirror
    pmgctl render-golden <dir>         M0  spike S1's native side
    pmgctl cas verify [--repair]       M1
    pmgctl chaos                       M2  spike S8, becomes a release gate
";

/// A crate's selftest: a name and a function that returns a specific failure.
///
/// `String` rather than a shared error type on purpose — a selftest failure is
/// read by a person on the machine that has the problem, so the message is the
/// whole payload.
type Suite = (&'static str, fn() -> Result<(), String>);

/// Every crate that has assertions worth gating a release on.
///
/// A crate whose selftest is trivially `Ok(())` today is listed anyway, so that
/// adding real assertions to it later is a one-line change rather than a
/// question of where they get wired in.
fn suites() -> Vec<Suite> {
    vec![
        ("pmg-scene", pmg_scene::selftest),
        ("pmg-layout", pmg_layout::selftest),
        ("pmg-text", pmg_text::selftest),
        ("pmg-anim", pmg_anim::selftest),
        ("pmg-render", pmg_render::selftest),
        ("pmg-color", pmg_color::selftest),
        ("pmg-media", pmg_media::selftest),
        ("pmg-audio", pmg_audio::selftest),
        ("pmg-cas", pmg_cas::selftest),
        ("pmg-proto", pmg_proto::selftest),
    ]
}

fn selftest() -> Result<()> {
    let mut failures = Vec::new();

    for (name, run) in suites() {
        match run() {
            Ok(()) => println!("  ok    {name}"),
            Err(e) => {
                println!("  FAIL  {name}: {e}");
                failures.push(format!("{name}: {e}"));
            }
        }
    }

    if !failures.is_empty() {
        eprintln!();
        for f in &failures {
            eprintln!("FAILED: {f}");
        }
        bail!("{} suite(s) failed", failures.len());
    }

    // The word CI greps for. Keep it on its own line and keep it last.
    println!("PASS");
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--selftest") => selftest(),
        Some("--version") => {
            println!("pmgctl {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some("--help") | Some("-h") | None => {
            print!("{USAGE}");
            Ok(())
        }
        Some(other) => {
            eprintln!("unknown argument: {other}\n");
            print!("{USAGE}");
            std::process::exit(2);
        }
    }
}
