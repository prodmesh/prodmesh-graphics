//! The PMG Link client. Spec: `docs/specs/pmg-link.md`.
//!
//! **The node dials the server.** One blocking WebSocket on its own thread,
//! CBOR frames, and a channel to whoever owns the layer engine.
//!
//! Blocking rather than async, deliberately: the node is thread-per-output-group
//! by design (ADR 0011) because that is how the plant is wired, and putting a
//! task scheduler between a cue and a graphic buys nothing here.
//!
//! # The ordering that is the whole autonomy invariant
//!
//! `AssignmentAck` is sent **only after** `store.save_assignment` returns, and
//! that function returns only after an fsync. Reverse the two and a node can
//! ack something it will forget on the next power cut — with no symptom at all
//! until the morning it matters (ADR 0009).
//!
//! # Losing the server is not an error
//!
//! A dropped link means reconnect with backoff and **keep playing**. Nothing in
//! here evicts a layer, clears an output, or touches the engine on disconnect.
//! If you ever find code here that does, it is a bug in the deepest sense.

use anyhow::{Context, Result};
use pmg_proto::{ToNode, ToServer};
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tungstenite::Message;

/// What the link hands to the engine owner.
pub enum FromLink {
    Connected {
        epoch: u64,
    },
    /// A periodic nudge asking the owner for a fresh `StateReport`. Distinct
    /// from `Connected` because "we linked" and "a second passed" are different
    /// events, and folding them together made the owner re-run handshake work
    /// once a second.
    Tick,
    Disconnected,
    Assignment(Box<pmg_proto::Assignment>),
    Cue(Box<pmg_proto::Cue>),
    Resync(String),
}

/// What the engine owner hands back.
pub enum ToLink {
    Ack(pmg_proto::CueAck),
    State(pmg_proto::StateReport),
    Ready(pmg_proto::Ready),
    Progress(pmg_proto::SyncProgress),
    /// Sent once the assignment is durably on disk. The link never decides this
    /// for itself — only the owner knows the write returned.
    AssignmentSaved {
        gen: u64,
    },
}

pub struct Config {
    pub server: String,
    pub identity_node_id: String,
    pub identity_boot_id: String,
    pub name: String,
    pub version: String,
    pub capabilities: pmg_proto::Capabilities,
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Run the link forever. Returns only if the outbound channel closes.
pub fn run(cfg: Config, tx: Sender<FromLink>, rx: Receiver<ToLink>) {
    let mut backoff = Duration::from_millis(250);
    loop {
        match session(&cfg, &tx, &rx) {
            Ok(()) => backoff = Duration::from_millis(250),
            Err(e) => {
                tracing::warn!(error = %e, "link session ended");
            }
        }
        if tx.send(FromLink::Disconnected).is_err() {
            return;
        }
        std::thread::sleep(backoff);
        // Capped at five seconds. A node that has been away for an hour must
        // still come back promptly when the switch is plugged in — nobody is
        // watching a log to notice it waiting.
        backoff = (backoff * 2).min(Duration::from_secs(5));
    }
}

fn session(cfg: &Config, tx: &Sender<FromLink>, rx: &Receiver<ToLink>) -> Result<()> {
    let url = format!("ws://{}/link", cfg.server.trim_start_matches("ws://"));
    let (mut ws, _) = tungstenite::connect(&url).with_context(|| format!("dialling {url}"))?;
    // A short read timeout turns the blocking read into a poll, so this one
    // thread can also drain the outbound channel. Without it a node with
    // nothing to receive would never send its state heartbeat.
    if let tungstenite::stream::MaybeTlsStream::Plain(tcp) = ws.get_ref() {
        tcp.set_read_timeout(Some(Duration::from_millis(20))).ok();
    }

    send(
        &mut ws,
        &ToServer::Hello(pmg_proto::Hello {
            proto_ver: pmg_proto::PROTO_VER,
            node_id: cfg.identity_node_id.clone(),
            boot_id: cfg.identity_boot_id.clone(),
            name: cfg.name.clone(),
            version: cfg.version.clone(),
            assignment_gen: None,
            last_applied_seq: 0,
            capabilities: cfg.capabilities.clone(),
        }),
    )?;

    let mut pending_gen: Option<u64> = None;
    let mut last_state = Instant::now();

    loop {
        // --- inbound ------------------------------------------------------
        match ws.read() {
            Ok(Message::Binary(buf)) => match pmg_proto::decode::<ToNode>(&buf) {
                Ok(msg) => {
                    if let Some(gen) = handle(msg, &mut ws, tx)? {
                        pending_gen = Some(gen);
                    }
                }
                // A frame we cannot parse is not a reason to drop a link that
                // may be holding graphics on air.
                Err(e) => tracing::warn!(error = %e, "undecodable frame"),
            },
            Ok(Message::Close(_)) => return Ok(()),
            Ok(_) => {}
            Err(tungstenite::Error::Io(e))
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(e) => return Err(e.into()),
        }

        // --- outbound -----------------------------------------------------
        while let Ok(out) = rx.try_recv() {
            match out {
                ToLink::Ack(a) => send(&mut ws, &ToServer::CueAck(a))?,
                ToLink::State(s) => send(&mut ws, &ToServer::StateReport(s))?,
                ToLink::Ready(r) => send(&mut ws, &ToServer::Ready(r))?,
                ToLink::Progress(p) => send(&mut ws, &ToServer::SyncProgress(p))?,
                ToLink::AssignmentSaved { gen } => {
                    // HERE, and only here. The owner has fsynced.
                    send(&mut ws, &ToServer::AssignmentAck { gen })?;
                    pending_gen = pending_gen.filter(|g| *g != gen);
                }
            }
        }

        // A 1 s state heartbeat, so the Show view greys a node's layers within
        // seconds rather than waiting for TCP to give up — which over failing
        // wifi can take half a minute.
        if last_state.elapsed() >= Duration::from_millis(pmg_proto::PING_INTERVAL_MS) {
            last_state = Instant::now();
            if tx.send(FromLink::Tick).is_err() {
                return Ok(());
            }
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// Returns `Some(gen)` when an assignment arrived and is awaiting a durable save.
fn handle(
    msg: ToNode,
    ws: &mut tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<std::net::TcpStream>>,
    tx: &Sender<FromLink>,
) -> Result<Option<u64>> {
    match msg {
        ToNode::Welcome(w) => {
            tracing::info!(epoch = w.epoch, server = %w.server_id, "linked");
            let _ = tx.send(FromLink::Connected { epoch: w.epoch });
        }
        ToNode::AssignmentOffer { gen, .. } => {
            // M1 always asks for the body. The digest exists so an already
            // current node can answer AssignmentOk with no transfer; wiring
            // that needs the node to remember the digest it last saved, which
            // is M2's reconciler.
            send(ws, &ToServer::AssignmentNeed { gen })?;
        }
        ToNode::Assignment(a) => {
            let gen = a.gen;
            let _ = tx.send(FromLink::Assignment(Box::new(a)));
            return Ok(Some(gen));
        }
        ToNode::Cue(c) => {
            let _ = tx.send(FromLink::Cue(Box::new(c)));
        }
        ToNode::Resync { reason } => {
            let _ = tx.send(FromLink::Resync(reason));
        }
        ToNode::Ping { t_ms } => send(ws, &ToServer::Pong { t_ms })?,
    }
    Ok(None)
}

fn send<S: std::io::Read + std::io::Write>(
    ws: &mut tungstenite::WebSocket<S>,
    msg: &ToServer,
) -> Result<()> {
    let bytes = pmg_proto::encode(msg).map_err(|e| anyhow::anyhow!("{e}"))?;
    ws.send(Message::Binary(bytes.into()))?;
    Ok(())
}
