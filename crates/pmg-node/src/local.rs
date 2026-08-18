//! The node's own control API on `:8520` — the emergency surface.
//!
//! Spec: `docs/specs/control-surfaces.md` §2. Decision: ADR 0014.
//!
//! # Why the node has an API at all
//!
//! Because the server can be dead. GPI, OSC and MIDI are physical signals from
//! inside the building, and routing them through a control server would make
//! them useless in exactly the failure they exist to cover. They terminate
//! here. Companion and RossTalk can be pointed here too, as a fallback.
//!
//! The roadmap's verification for the whole of M4 is one line — *every surface
//! fires a take with the control server stopped* — and this is what makes that
//! sentence true rather than aspirational.
//!
//! # It has no path of its own into the engine
//!
//! A cue arriving here is put on the **same channel** the link uses and applied
//! by the same loop, with the same dedupe, the same deadline rule, and the same
//! superseding. Only the reply address differs. That is the rule from
//! `control-surfaces.md` §1 — *no adapter gets its own cue semantics, its own
//! state model, or its own path into the layer engine* — and an adapter that
//! reached into the engine directly is how that rule would quietly stop being
//! true.
//!
//! # Deliberately tiny, and deliberately not authenticated
//!
//! Hand-rolled HTTP/1.1 over `TcpListener`: a handful of routes, no framework,
//! no async runtime. The node already runs a thread per output group and a
//! blocking link thread; a scheduler here would buy nothing.
//!
//! **There is no auth.** This is a booth LAN with no internet (the whole family
//! assumes that), and the surface exists for the moment the server is gone —
//! a credential check against a server that is down is a lock with no key. It
//! binds to the machine's interfaces because a GPI box on the same switch has
//! to reach it. If this ever leaves a trusted network that is an ADR, not a
//! flag.

use anyhow::Result;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::Sender;

use crate::link::FromLink;

/// How long to wait for the engine to answer a locally-submitted cue.
///
/// Generous next to a cue's own deadline: this covers the engine being busy
/// arming, and the cue carries its own deadline for lateness. Returning "no
/// answer" while the take is in flight would be the take button lying, one
/// layer down.
const REPLY_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(2_000);

/// Serve forever. Runs on its own thread.
pub fn serve(port: u16, tx: Sender<FromLink>, state: StateHandle) -> Result<()> {
    let listener = TcpListener::bind(("0.0.0.0", port))?;
    tracing::info!(port, "node API listening — works with the server down");
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let tx = tx.clone();
        let state = state.clone();
        // A thread per connection. These are single request/response exchanges
        // from a GPI box or a panel, not a stream; a pool would be machinery
        // for a load that does not exist.
        std::thread::spawn(move || {
            if let Err(e) = handle(stream, &tx, &state) {
                tracing::debug!(error = %e, "node API connection ended");
            }
        });
    }
    Ok(())
}

/// Read-only access to what is on air, for the status route.
pub type StateHandle = std::sync::Arc<std::sync::Mutex<crate::engine::LayerEngine>>;

fn handle(mut stream: TcpStream, tx: &Sender<FromLink>, state: &StateHandle) -> Result<()> {
    stream.set_read_timeout(Some(std::time::Duration::from_secs(5)))?;
    let mut reader = BufReader::new(stream.try_clone()?);

    let mut request = String::new();
    reader.read_line(&mut request)?;
    let mut parts = request.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("/").to_string();

    let mut length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 || line.trim().is_empty() {
            break;
        }
        if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            length = v.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0u8; length];
    if length > 0 {
        reader.read_exact(&mut body)?;
    }

    let (status, content_type, payload) = match (method.as_str(), path.as_str()) {
        ("POST", "/cue") => {
            let (code, json) = cue(&body, tx);
            (code, "application/json", json)
        }
        ("GET", "/status") => (200, "application/json", status_json(state)),
        ("GET", "/") => (200, "text/html; charset=utf-8", status_page(state)),
        ("GET", "/health") => (200, "application/json", "{\"ok\":true}".to_string()),
        _ => (
            404,
            "application/json",
            "{\"error\":\"no_such_route\"}".to_string(),
        ),
    };

    write!(
        stream,
        "HTTP/1.1 {status} {}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\
         Cache-Control: no-store\r\nConnection: close\r\n\r\n{payload}",
        reason(status),
        payload.len(),
    )?;
    stream.flush()?;
    Ok(())
}

fn reason(code: u16) -> &'static str {
    match code {
        200 => "OK",
        404 => "Not Found",
        422 => "Unprocessable Entity",
        503 => "Service Unavailable",
        _ => "Bad Request",
    }
}

// ---------------------------------------------------------------------------
// The cue route
// ---------------------------------------------------------------------------

/// Accept a cue and answer with what the engine did.
///
/// The body is the SAME shape `POST /api/cue` takes on the server, so an
/// adapter written against one works against the other by changing a host.
/// That is the point of §1's rule: a Companion button pointed at the node in an
/// emergency sends the identical JSON it sends to the server every Sunday.
fn cue(body: &[u8], tx: &Sender<FromLink>) -> (u16, String) {
    let Ok(req) = serde_json::from_slice::<CueRequest>(body) else {
        return (422, err("validation_failed", "That is not a cue this node understands."));
    };
    // Generated where the human acted, never invented here. A surface that
    // mints an id per retry has defeated the dedupe, and inventing one on this
    // side would hide that rather than surface it.
    if req.cue_id.trim().is_empty() {
        return (422, err("validation_failed", "This cue has no cue_id."));
    }
    let Some(op) = req.to_op() else {
        return (422, err("validation_failed", "There is no cue by that name."));
    };

    let (reply_tx, reply_rx) = std::sync::mpsc::channel();
    let cue = pmg_proto::Cue {
        cue_id: req.cue_id.clone(),
        // Local cues carry no sequence: `seq` orders the SERVER's stream and
        // dedupes a replay after a reconnect, and a number invented here would
        // collide with that stream rather than join it. Dedupe still holds,
        // because `cue_id` is the other half of the key and is the half a human
        // action owns.
        seq: 0,
        epoch: 0,
        deadline_ms: req.deadline_ms,
        at: None,
        op,
    };
    if tx
        .send(FromLink::Cue(
            Box::new(cue),
            std::time::Instant::now(),
            Some(reply_tx),
        ))
        .is_err()
    {
        return (503, err("node_stopping", "This node is shutting down."));
    }

    match reply_rx.recv_timeout(REPLY_TIMEOUT) {
        Ok(ack) => {
            let result = format!("{:?}", ack.result).to_lowercase();
            let code = match ack.result {
                pmg_proto::CueResult::Applied
                | pmg_proto::CueResult::Deduped
                | pmg_proto::CueResult::Superseded => 200,
                pmg_proto::CueResult::Rejected => 409,
            };
            let reason = ack
                .reason
                .map(|r| format!("\"{r:?}\""))
                .unwrap_or_else(|| "null".into());
            (
                code,
                format!(
                    "{{\"result\":\"{result}\",\"reason\":{reason},\"fired_at_frame\":{}}}",
                    ack.fired_at_frame
                        .map(|f| f.to_string())
                        .unwrap_or_else(|| "null".into())
                ),
            )
        }
        // The engine did not answer. Saying so beats a success this cannot
        // vouch for — the same rule the server's synchronous endpoint follows.
        Err(_) => (503, err("cue_not_acked", "The engine did not confirm the cue.")),
    }
}

fn err(code: &str, label: &str) -> String {
    format!("{{\"error\":\"{code}\",\"label\":\"{label}\"}}")
}

#[derive(serde::Deserialize)]
struct CueRequest {
    // Defaulted so a MISSING id reaches the check below and gets a sentence
    // about cue_id, rather than failing deserialization and getting the
    // generic "that is not a cue" — which sends an integrator looking at the
    // wrong field.
    #[serde(default)]
    cue_id: String,
    op: String,
    #[serde(default)]
    item: Option<String>,
    #[serde(default)]
    output: Option<String>,
    #[serde(default)]
    layer: Option<String>,
    #[serde(default)]
    deadline_ms: Option<u64>,
}

impl CueRequest {
    fn to_op(&self) -> Option<pmg_proto::CueOp> {
        let output = self.output.clone().unwrap_or_else(|| "out-0".into());
        let layer = self.layer.clone().unwrap_or_else(|| "Lower Third".into());
        let item = self.item.clone();
        Some(match self.op.as_str() {
            "take" => pmg_proto::CueOp::Take {
                item: item?,
                output,
                layer,
                destinations: Vec::new(),
            },
            "take_out" => pmg_proto::CueOp::TakeOut { output, layer },
            "clear_layer" => pmg_proto::CueOp::ClearLayer { output, layer },
            "clear_all" => pmg_proto::CueOp::ClearAll { output },
            "arm" => pmg_proto::CueOp::Arm {
                item: item?,
                output,
                layer,
            },
            // Deliberately a SUBSET. `update` needs bound data and `timeline`
            // needs a name and an action; both belong to a surface with a
            // keyboard, and a GPI contact closure has neither. Adding them
            // because the enum has them would be building for nobody.
            _ => return None,
        })
    }
}

// ---------------------------------------------------------------------------
// Status
// ---------------------------------------------------------------------------

/// What is on air, read from the engine.
///
/// The node is the authority (ADR 0009), so this is the answer rather than a
/// cache of one — which is exactly why it is worth having when the server, the
/// usual place to look, is the thing that has failed.
fn status_json(state: &StateHandle) -> String {
    let s = state.lock().unwrap().state_report();
    let outputs: Vec<String> = s
        .outputs
        .iter()
        .map(|o| {
            let layers: Vec<String> = o
                .layers
                .iter()
                .map(|l| {
                    let occ = match &l.occupant {
                        Some(x) => format!(
                            "{{\"item\":{},\"label\":{},\"phase\":\"{}\"}}",
                            json_str(&x.item_id),
                            json_str(&x.item_label),
                            x.phase
                        ),
                        None => "null".into(),
                    };
                    format!(
                        "{{\"name\":{},\"occupant\":{occ}}}",
                        json_str(&l.name)
                    )
                })
                .collect();
            format!(
                "{{\"id\":{},\"fps\":{:.2},\"late_frames\":{},\"layers\":[{}]}}",
                json_str(&o.id),
                o.fps,
                o.late_frames,
                layers.join(",")
            )
        })
        .collect();
    format!("{{\"outputs\":[{}]}}", outputs.join(","))
}

/// Minimal escaping — enough for the strings that reach here.
///
/// Node ids, layer names and Item labels. A label is operator-typed and can
/// hold a quote or a backslash; anything beyond that would mean writing a JSON
/// encoder, and this file already declined to take a dependency.
fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A page for the person standing at the machine.
///
/// No CSS framework, no fonts, no CDN — booth machines have no internet, and
/// this is read when something has already gone wrong. It says what is on air
/// and it refreshes itself, and that is the whole brief.
fn status_page(state: &StateHandle) -> String {
    let s = state.lock().unwrap().state_report();
    let mut rows = String::new();
    for o in &s.outputs {
        for l in &o.layers {
            let what = match &l.occupant {
                Some(x) => html(if x.item_label.is_empty() { &x.item_id } else { &x.item_label }),
                None => "<i>clear</i>".into(),
            };
            rows.push_str(&format!(
                "<tr><td>{}</td><td>{}</td><td>{what}</td></tr>",
                html(&o.id),
                html(&l.name)
            ));
        }
    }
    format!(
        "<!doctype html><meta charset=utf-8><meta http-equiv=refresh content=2>\
         <title>Playout node</title>\
         <style>body{{font:14px system-ui;background:#0b0f14;color:#e8eef5;padding:24px}}\
         table{{border-collapse:collapse}}td,th{{padding:6px 14px;border-bottom:1px solid #263141;text-align:left}}\
         i{{color:#7c8b9c}}</style>\
         <h1>On air</h1><table><tr><th>Output<th>Layer<th>Showing</tr>{rows}</table>\
         <p><i>This node is the authority for what is on air. It keeps playing \
         with the control server down.</i></p>"
    )
}

fn html(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}
