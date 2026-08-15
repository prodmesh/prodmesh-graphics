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
//! Concretely, that means:
//!
//! - An `Assignment` is acked **only after it is durably written to disk**.
//!   That ordering is the entire autonomy invariant in one line.
//! - With a valid `assignment.cbor` and a complete CAS, this process runs
//!   forever with no server.
//! - On boot with no server it comes up **black**. Auto-restore is an explicit
//!   per-output opt-in — an unattended machine re-firing a graphic after a power
//!   cut is a bad surprise.
//! - GPI, MIDI, OSC, and the local API on `:8520` terminate **here**, not at the
//!   server, because that is what makes them work in the failure they exist to
//!   cover (ADR 0014).
//!
//! # Threading
//!
//! One render thread per **output group**, pumped by that group's own frame
//! callback. An output group is a set of outputs sharing a clock, so the
//! threading model is determined by the plant's wiring. Two references means two
//! render loops (ADR 0011).
//!
//! # M0 status
//!
//! Prints its configuration and exits. Everything above is design, not code —
//! see `docs/ROADMAP.md`.

use anyhow::Result;

/// Node API and status page. The emergency surface when the server is dead.
const NODE_PORT: u16 = 8520;

const USAGE: &str = "\
pmg-node — ProdMesh Graphics playout node

USAGE:
    pmg-node [--server <host:port>] [--output null] [--state-dir <path>]

    --server      Control server to dial. Omit to rely on mDNS discovery.
                  Manual entry always works; mDNS is an aid, never an authority.
    --output      `null` renders without an output device, for tests.
    --state-dir   Where assignment.cbor and the local CAS live.
                  Default: platform data dir.
";

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print!("{USAGE}");
        return Ok(());
    }
    if args.iter().any(|a| a == "--version") {
        println!("pmg-node {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    println!(
        "pmg-node {} (M0 scaffold — renders nothing yet)",
        env!("CARGO_PKG_VERSION")
    );
    println!("  proto_ver        {}", pmg_proto::PROTO_VER);
    println!("  scene reader     v{}", pmg_scene::READER_MAJOR);
    println!("  node API port    {NODE_PORT}");
    println!("\nSee docs/ROADMAP.md — the link client, assignment reconciler,");
    println!("layer engine, and scheduler land in M1 and M2.");
    Ok(())
}
