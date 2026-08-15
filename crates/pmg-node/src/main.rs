//! `pmg-node` — the playout node.
//!
//! The process that must not fail. It runs unattended during a service, holds
//! the frame budget, and a crash in it is a black graphic in front of a
//! congregation.
//!
//! # The invariant this binary exists to hold
//!
//! **The node is the authority on what is on air.** The server is the authority
//! on content and assignment. A server restart must not change a pixel; on
//! reconnect the server *reads* our `StateReport` and adopts it (ADR 0009).
//!
//! Concretely:
//!
//! - An `Assignment` is acked **only after it is durably written to disk**.
//!   That ordering is the entire autonomy invariant in one line, and it lives
//!   in `runtime.rs` where the assignment arrives.
//! - With a valid `assignment.cbor` and a complete CAS, this process runs with
//!   no server.
//! - On boot with no server it comes up **black**. Auto-restore is not
//!   implemented — an unattended machine re-firing a graphic after a power cut
//!   is a bad surprise, and it must be an explicit opt-in when it exists.
//!
//! # Threading
//!
//! One render thread per **output group**, plus the link thread. An output
//! group is a set of outputs sharing a clock, so the threading model is
//! determined by the plant's wiring: two references means two render loops
//! (ADR 0011). M1 has one output, so the frame loop is the main thread.
//!
//! # M1 scope, stated plainly
//!
//! One NDI output, one layer stack, no preview channel, no local control
//! surfaces, no mDNS. `--server` is required. Those are M2.

use anyhow::{Context, Result};
use pmg_node::{link, runtime, store};

/// Node API and status page. The emergency surface when the server is dead —
/// M2, and deliberately not faked here.
#[allow(dead_code)]
const NODE_PORT: u16 = 8520;

const USAGE: &str = "\
pmg-node — ProdMesh Graphics playout node

USAGE:
    pmg-node --server <host:port> [options]

    --server <host:port>   Control server to dial. Required in M1; mDNS
                           discovery is M2. Manual entry always works, and
                           must — multicast is unreliable across VLANs.
    --state-dir <path>     Where identity, assignment, and the CAS live.
                           Default ./var/node
    --name <name>          Display name. Default: this machine's hostname.
    --output <id>          Output id. Default out-0
    --layers <a,b,c>       Layer names, in order. Default \"Lower Third\"
    --size <WxH>           Default 1920x1080
    --fps <N/D>            Default 60000/1001
    --ndi-name <name>      Default \"ProdMesh Graphics\"
    --font <file.ttf>      A face to use when the assignment carries no font
                           asset. Provisional — see runtime.rs.
    --headless             Render and time frames, but send nothing.
";

fn arg(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn main() -> Result<()> {
    tracing_subscriber_init();
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.iter().any(|a| a == "--help" || a == "-h") {
        print!("{USAGE}");
        return Ok(());
    }
    if args.iter().any(|a| a == "--selftest") {
        pmg_node::selftest().map_err(|e| anyhow::anyhow!(e))?;
        println!("PASS");
        return Ok(());
    }

    let server = arg(&args, "--server").context(
        "--server is required. mDNS discovery lands in M2, and manual entry must work \
         everywhere discovery appears anyway — multicast is genuinely unreliable across \
         VLANs and behind switches with poor IGMP snooping",
    )?;
    let state_dir = arg(&args, "--state-dir").unwrap_or_else(|| "var/node".into());
    let store = store::Store::open(&state_dir)?;
    let identity = store.identity(arg(&args, "--name"))?;

    let (w, h) = match arg(&args, "--size") {
        Some(s) => {
            let (a, b) = s.split_once('x').context("--size looks like 1920x1080")?;
            (a.parse()?, b.parse()?)
        }
        None => (1920u32, 1080u32),
    };
    let fps = match arg(&args, "--fps") {
        Some(s) => {
            let (a, b) = s.split_once('/').context("--fps looks like 60000/1001")?;
            (a.parse()?, b.parse()?)
        }
        None => (60000u32, 1001u32),
    };
    let layers: Vec<String> = arg(&args, "--layers")
        .unwrap_or_else(|| "Lower Third".into())
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    let fallback_font = match arg(&args, "--font") {
        Some(p) => Some(std::fs::read(&p).with_context(|| format!("reading {p}"))?),
        None => None,
    };

    let opts = runtime::Options {
        output_id: arg(&args, "--output").unwrap_or_else(|| "out-0".into()),
        layers,
        fps,
        ndi_name: arg(&args, "--ndi-name").unwrap_or_else(|| "ProdMesh Graphics".into()),
        width: w,
        height: h,
        server_http: format!("http://{server}"),
        fallback_font,
        headless: args.iter().any(|a| a == "--headless"),
    };

    println!("pmg-node  {}  ({})", identity.name, identity.node_id);
    println!("  server     {server}");
    println!("  state      {state_dir}");
    println!(
        "  output     {} {}x{} @ {}/{}",
        opts.output_id, w, h, fps.0, fps.1
    );
    println!("  layers     {}", opts.layers.join(", "));

    let mut rt = runtime::Runtime::new(store::Store::open(&state_dir)?, opts);

    // Adopt whatever is already on disk BEFORE dialling. This is the path that
    // runs when the server is down, and it is the same code the online path
    // uses — a separate offline path would only ever be exercised during an
    // outage, which is the worst time to discover a bug in it.
    if let Some(a) = store::Store::open(&state_dir)?.load_assignment() {
        println!(
            "  assignment gen {} from disk ({} items)",
            a.gen,
            a.items.len()
        );
        rt.adopt(&a, None);
    } else {
        println!("  assignment none yet");
    }
    println!("  boot state black (auto-restore is a deliberate non-feature)\n");

    let (link_tx, rt_rx) = std::sync::mpsc::channel();
    let (rt_tx, link_rx) = std::sync::mpsc::channel();

    let cfg = link::Config {
        server: server.clone(),
        identity_node_id: identity.node_id.clone(),
        identity_boot_id: identity.boot_id.clone(),
        name: identity.name.clone(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        capabilities: capabilities(&rt),
    };
    std::thread::Builder::new()
        .name("pmg-link".into())
        .spawn(move || link::run(cfg, link_tx, link_rx))?;

    rt.run(rt_rx, rt_tx)
}

/// Capabilities are **discovered, never configured**.
///
/// A capability an operator typed in is a capability that is wrong the day the
/// card moves — and a DeckLink read through a mismatched SDK reports defaults
/// rather than errors (`docs/INTEGRATION-NOTES.md`), so the honest source is a
/// query, every boot.
fn capabilities(_rt: &runtime::Runtime) -> pmg_proto::Capabilities {
    pmg_proto::Capabilities {
        // NDI has no enumerable hardware; a real DeckLink probe lands with
        // pmg-out-decklink in M2. Reporting a fabricated output list here would
        // be worse than reporting none.
        outputs: Vec::new(),
        gpu: None,
        decoders: Vec::new(),
        max_layers: 8,
        scene_reader_major: pmg_proto::SCENE_READER_MAJOR,
    }
}

fn tracing_subscriber_init() {
    // A real subscriber, to stderr, so the node's stdout stays readable when it
    // is run by hand in a booth.
    //
    // This started life as a no-op stub, and the cost of that was immediate:
    // the node exited on server loss and there was NOTHING in the log to say
    // why, because every tracing::warn! in the crate was going nowhere. A
    // process that must run unattended has to be able to say what happened to
    // it — that is not a nicety, it is the difference between a diagnosable
    // Sunday and a mystery.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("pmg_node=info,warn")),
        )
        .with_target(false)
        .with_writer(std::io::stderr)
        .init();
}
