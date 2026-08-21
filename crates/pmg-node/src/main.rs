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
use pmg_node::{link, local, runtime, store};

/// Node API and status page — the emergency surface when the server is dead
/// (`docs/specs/control-surfaces.md` §2). GPI, OSC and MIDI terminate here,
/// and Companion or RossTalk can be pointed here as a fallback.
const NODE_PORT: u16 = 8520;

/// OSC in, when `<state-dir>/surfaces.json` maps any addresses. Family 85xx
/// band, like everything else here — OSC has no registered port and desks let
/// you type one.
const OSC_PORT: u16 = 8522;

/// RossTalk in. Line-oriented TCP, always listening — a switcher pointed here
/// as its fallback must not depend on a mapping file existing
/// (`control-surfaces.md` §5).
const ROSSTALK_PORT: u16 = 8524;

const USAGE: &str = "\
pmg-node — ProdMesh Graphics playout node

USAGE:
    pmg-node --server <host:port> [options]

    --server <host:port>   Control server to dial. Omitted, the node browses
                           for one over mDNS. Manual entry always wins and
                           always works — multicast is unreliable across VLANs
                           and behind switches with poor IGMP snooping.
    --state-dir <path>     Where identity, assignment, and the CAS live.
                           Default ./var/node
    --name <name>          Display name. Default: this machine's hostname.
    --output <spec>        Repeatable. key=value,key=value — keys: id, backend,
                           group, name, mode, device, key, preview, audio
                           (embedded channel count), layers
                           (semicolon-separated), size, fps. Replaces the plain
                           output flags entirely when given.
                           e.g. --output id=sdi,backend=decklink,device=1,key=3,group=ref
    --output-id <id>       Id for the single default output. Default out-0
    --layers <a,b,c>       Layer names, in order. Default \"Lower Third\"
    --size <WxH>           Default 1920x1080
    --fps <N/D>            Default 60000/1001
    --ndi-name <name>      Default \"ProdMesh Graphics\"
    --backend <kind>       ndi (default) | decklink
    --decklink <index>     Device index for the fill. See `pmgctl decklink list`
    --decklink-key <index> Second port for the key half of a fill+key pair.
                           Omit for fill only.
    --decklink-mode <name> Display mode name, e.g. 1080p59.94
    --font <file.ttf>      A face to use when the assignment carries no font
                           asset. Provisional — see runtime.rs.
    --headless             Render and time frames, but send nothing.
    --api-port <n>         The node's own control API and status page.
                           Default 8520. This is the surface that still works
                           when the control server is down, so GPI, OSC and
                           MIDI terminate here.
    --osc-port <n>         OSC in. Default 8522. Only listens when
                           <state-dir>/surfaces.json maps OSC addresses.
    --rosstalk-port <n>    RossTalk in. Default 8524. GPI/CC resolve through
                           <state-dir>/surfaces.json; CUSTOM needs no mapping.
    --preview-port <n>     Preview channel. Default 8521; 0 disables it.
    --inject-stall-at <s>  TEST INSTRUMENT. Stall the render thread for one
                           second, once, this many seconds in — so audio-sync
                           recovery can be asserted rather than waited for.
    --preview-mjpeg        Serve MJPEG over HTTP instead of H.264 over a
                           WebSocket. Needs no decoder in the browser, so it is
                           the fallback when H.264 shows nothing.
";

/// Parse one `--output key=value,key=value` spec.
///
/// Unknown keys are an ERROR, not ignored. A typo'd `preveiw=8521` that
/// silently leaves the preview on its default is the kind of thing discovered
/// during a service.
fn parse_output(
    spec: &str,
    index: usize,
    default_layers: &[String],
    default_fps: (u32, u32),
    default_w: u32,
    default_h: u32,
) -> Result<runtime::OutputSpec> {
    let mut out = runtime::OutputSpec {
        id: format!("out-{index}"),
        layers: default_layers.to_vec(),
        fps: default_fps,
        width: default_w,
        height: default_h,
        backend: "ndi".into(),
        ndi_name: format!("ProdMesh Graphics {index}"),
        decklink_device: 0,
        decklink_key_device: None,
        decklink_mode: "1080p59.94".into(),
        preview_port: pmg_preview::PREVIEW_PORT + index as u16,
        // Its own group unless told otherwise: assuming two outputs share a
        // clock when they do not would pace one against a reference it does not
        // follow (ADR 0011).
        group: format!("g{index}"),
        // Silent unless asked. A lower-third key feed carrying audio would put
        // the same sound on a second SDI feed a plant may well sum.
        audio_channels: 0,
        preview_mjpeg: false,
        inject_stall_at_s: None,
    };

    for pair in spec.split(',') {
        let pair = pair.trim();
        if pair.is_empty() {
            continue;
        }
        let (k, v) = pair
            .split_once('=')
            .with_context(|| format!("{pair:?} is not key=value in --output {spec:?}"))?;
        match k.trim() {
            "id" => out.id = v.into(),
            "backend" => out.backend = v.into(),
            "group" => out.group = v.into(),
            "name" => out.ndi_name = v.into(),
            "mode" => out.decklink_mode = v.into(),
            "device" => out.decklink_device = v.parse().context("device must be a number")?,
            "key" => out.decklink_key_device = Some(v.parse().context("key must be a number")?),
            "preview" => out.preview_port = v.parse().context("preview must be a port")?,
            "audio" => out.audio_channels = v.parse().context("audio must be a channel count")?,
            // Semicolons, because commas already separate the pairs.
            "layers" => {
                out.layers = v
                    .split(';')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            }
            "size" => {
                let (a, b) = v.split_once('x').context("size looks like 1920x1080")?;
                out.width = a.parse()?;
                out.height = b.parse()?;
            }
            "fps" => {
                let (a, b) = v.split_once('/').context("fps looks like 60000/1001")?;
                out.fps = (a.parse()?, b.parse()?);
            }
            other => anyhow::bail!(
                "unknown --output key {other:?}. Known: id, backend, group, name, mode, \
                 device, key, preview, audio, layers, size, fps"
            ),
        }
    }
    Ok(out)
}

fn arg(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

/// The HTTP base for CAS transfers, from whatever `--server` was given.
///
/// **Assets never travel over the control socket** (ADR 0007), so the node needs
/// an HTTP origin alongside the WebSocket one. `--server` is documented as a
/// `host:port`, but `ws://host:port` is the obvious thing to type and the two
/// must not disagree.
///
/// Blindly prefixing produced `http://ws://127.0.0.1:8519/cas/...`, and the
/// symptom was not a bad URL in a log anyone read: the node linked, reported
/// healthy, took its assignment, and simply never became ready — because every
/// staging fetch failed and an un-armable Item is a *designed* state, not an
/// error. It looked like a node that had nothing to play.
fn http_base(server: &str) -> String {
    let bare = server
        .strip_prefix("ws://")
        .or_else(|| server.strip_prefix("wss://"))
        .or_else(|| server.strip_prefix("http://"))
        .or_else(|| server.strip_prefix("https://"))
        .unwrap_or(server);
    // wss:// implies TLS on the control socket, so assets go over TLS too —
    // anything else would be a downgrade nobody asked for.
    let scheme = if server.starts_with("wss://") || server.starts_with("https://") {
        "https"
    } else {
        "http"
    };
    format!("{scheme}://{}", bare.trim_end_matches('/'))
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

    // Manual entry first, always. Discovery is what happens when nobody typed
    // an address — never something that can override one.
    let server = match arg(&args, "--server") {
        Some(s) => s,
        None => {
            println!("  looking for a control server on this network…");
            let (found, all) = pmg_node::discover::find(std::time::Duration::from_secs(5))
                .context(
                    "no control server found on this network, and no --server given.\n\
                 Multicast is unreliable across VLANs and behind switches with poor IGMP \
                 snooping, so this is a normal thing to hit — pass --server <host:port>.",
                )?;
            if all.len() > 1 {
                // Reported, not silently resolved. Two control servers on one
                // network is a misconfiguration a node cannot arbitrate.
                println!(
                    "  WARNING: {} control servers answered: {}",
                    all.len(),
                    all.join(", ")
                );
                println!("  using {found}. Pass --server to choose.");
            }
            found
        }
    };
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

    // One output from the plain flags, or several from repeated --output.
    //
    // The plain flags stay because a single NDI output is the common case and
    // `--output id=…,backend=…` for it would be ceremony. They describe output
    // zero; --output replaces them entirely rather than layering on top, so
    // there is never a question about which one won.
    let specs: Vec<String> = args
        .iter()
        .enumerate()
        .filter(|(_, a)| a.as_str() == "--output")
        .filter_map(|(i, _)| args.get(i + 1).cloned())
        .collect();

    let outputs = if specs.is_empty() {
        vec![runtime::OutputSpec {
            id: arg(&args, "--output-id").unwrap_or_else(|| "out-0".into()),
            layers,
            fps,
            width: w,
            height: h,
            backend: if args.iter().any(|a| a == "--headless") {
                "headless".into()
            } else {
                arg(&args, "--backend").unwrap_or_else(|| "ndi".into())
            },
            ndi_name: arg(&args, "--ndi-name").unwrap_or_else(|| "ProdMesh Graphics".into()),
            decklink_device: arg(&args, "--decklink")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0),
            decklink_key_device: arg(&args, "--decklink-key").and_then(|s| s.parse().ok()),
            decklink_mode: arg(&args, "--decklink-mode").unwrap_or_else(|| "1080p59.94".into()),
            audio_channels: arg(&args, "--audio-channels")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0),
            preview_mjpeg: args.iter().any(|a| a == "--preview-mjpeg"),
            inject_stall_at_s: arg(&args, "--inject-stall-at").and_then(|s| s.parse().ok()),
            preview_port: arg(&args, "--preview-port")
                .and_then(|s| s.parse().ok())
                .unwrap_or(pmg_preview::PREVIEW_PORT),
            group: "default".into(),
        }]
    } else {
        specs
            .iter()
            .enumerate()
            .map(|(i, spec)| parse_output(spec, i, &layers, fps, w, h))
            .collect::<Result<Vec<_>>>()?
    };

    println!("pmg-node  {}  ({})", identity.name, identity.node_id);
    println!("  server     {server}");
    println!("  state      {state_dir}");
    for o in &outputs {
        println!(
            "  output     {} {}x{} @ {}/{} · {} · group {} · layers {}",
            o.id,
            o.width,
            o.height,
            o.fps.0,
            o.fps.1,
            o.backend,
            o.group,
            o.layers.join(", ")
        );
    }

    // Captured before the specs are handed to the runtime — the server is told
    // where this node's preview listens, and the first output is the pointer.
    let first_preview_port = outputs.first().map(|o| o.preview_port).unwrap_or(0);

    let opts = runtime::Options {
        outputs,
        server_http: http_base(&server),
        fallback_font,
    };

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

    let node_port: u16 = arg(&args, "--api-port")
        .and_then(|v| v.parse().ok())
        .unwrap_or(NODE_PORT);

    let osc_port: u16 = arg(&args, "--osc-port")
        .and_then(|v| v.parse().ok())
        .unwrap_or(OSC_PORT);

    let rosstalk_port: u16 = arg(&args, "--rosstalk-port")
        .and_then(|v| v.parse().ok())
        .unwrap_or(ROSSTALK_PORT);

    let (link_tx, rt_rx) = std::sync::mpsc::channel();
    let (rt_tx, link_rx) = std::sync::mpsc::channel();

    let cfg = link::Config {
        server: server.clone(),
        identity_node_id: identity.node_id.clone(),
        identity_boot_id: identity.boot_id.clone(),
        name: identity.name.clone(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        capabilities: capabilities(&rt),
        api_port: node_port,
        // The FIRST output's preview port. With several outputs each has its
        // own, and the node's status page is where a person finds the rest —
        // one number in a header is a pointer, not an inventory.
        preview_port: first_preview_port,
    };
    // GPI, OSC and MIDI — the surfaces that terminate HERE (ADR 0014).
    //
    // Read from the node's own disk, because a mapping fetched from the server
    // is a mapping that is missing on the morning the server is down.
    let mapping = pmg_node::surface::Mapping::load(std::path::Path::new(&state_dir))?;
    if !mapping.osc.is_empty() {
        let osc_tx = link_tx.clone();
        let m = mapping.clone();
        std::thread::Builder::new()
            .name("pmg-osc".into())
            .spawn(move || {
                if let Err(e) = pmg_node::surface::serve_osc(osc_port, m, osc_tx) {
                    tracing::error!(port = osc_port, error = %e, "OSC listener did not start");
                }
            })?;
    }

    // RossTalk, always on — a Ross switcher or a DashBoard panel is pointed
    // here as its fallback when the server is down, and a listener that only
    // appears once someone has written a mapping file is one that is missing
    // in exactly that case. `CUSTOM` needs no mapping at all.
    {
        let rt_tx2 = link_tx.clone();
        let m = mapping.clone();
        std::thread::Builder::new()
            .name("pmg-rosstalk".into())
            .spawn(move || {
                if let Err(e) = pmg_node::surface::serve_rosstalk(rosstalk_port, m, rt_tx2) {
                    tracing::error!(port = rosstalk_port, error = %e, "RossTalk did not start");
                }
            })?;
    }

    // The node's own API, on the same channel the link uses. Started BEFORE
    // the link, because its whole reason to exist is the case where the link
    // never comes up.
    let api_tx = link_tx.clone();
    let api_state = rt.engine().clone();
    std::thread::Builder::new()
        .name("pmg-api".into())
        .spawn(move || {
            if let Err(e) = local::serve(node_port, api_tx, api_state) {
                // Loud, and not fatal. A port already in use must not stop a
                // node putting graphics on air — the API is the fallback
                // surface, not the reason the machine is here.
                tracing::error!(port = node_port, error = %e, "node API did not start");
            }
        })?;

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
    // No colour when nobody is watching a terminal. An unattended node's log is
    // a FILE, read later by a person or a harness, and escape codes in it break
    // every regex written against it — a soak once reported PASS on four
    // metrics it had found no data for, because `late=` never appeared
    // literally between the colour codes.
    let ansi = std::io::IsTerminal::is_terminal(&std::io::stdout());
    tracing_subscriber::fmt()
        .with_ansi(ansi)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("pmg_node=info,warn")),
        )
        .with_target(false)
        .with_writer(std::io::stderr)
        .init();
}

#[cfg(test)]
mod tests {
    use super::http_base;

    #[test]
    fn a_server_url_with_a_scheme_is_not_prefixed_again() {
        assert_eq!(http_base("127.0.0.1:8519"), "http://127.0.0.1:8519");
        assert_eq!(http_base("ws://127.0.0.1:8519"), "http://127.0.0.1:8519");
        assert_eq!(http_base("http://127.0.0.1:8519"), "http://127.0.0.1:8519");
        assert_eq!(
            http_base("ws://booth-1.local:8519/"),
            "http://booth-1.local:8519"
        );
    }

    /// A TLS control socket must not silently fetch assets in the clear.
    #[test]
    fn tls_on_the_socket_means_tls_for_assets() {
        assert_eq!(
            http_base("wss://gfx.example:8519"),
            "https://gfx.example:8519"
        );
        assert_eq!(
            http_base("https://gfx.example:8519"),
            "https://gfx.example:8519"
        );
    }
}
