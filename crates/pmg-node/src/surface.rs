//! GPI, OSC and MIDI — the surfaces that terminate at the node.
//!
//! Spec: `docs/specs/control-surfaces.md` §2. Decision: ADR 0014.
//!
//! # Why these three are different from the others
//!
//! Companion and RossTalk are network clients that can be pointed at either
//! host. These three are **physical or console-local signals from inside the
//! building** — a contact closure, a fader, a footswitch — and routing them
//! through a control server would make them useless in exactly the failure
//! they exist to cover. They terminate here, and they submit through the same
//! channel as everything else so they inherit dedupe, deadlines and
//! superseding rather than reimplementing them.
//!
//! # The node mints the cue id, and that is not a contradiction
//!
//! Everywhere else the rule is that `cue_id` is generated where the human
//! acted. For these surfaces **the node IS where the human acted** — the edge
//! arrived on a wire in this building. So it is minted here, and the retry
//! problem the rule protects against becomes a different one: contact bounce.
//! A relay closing can produce a burst of edges over a few milliseconds, and
//! each would be a genuinely distinct action by the id rule. [`Debounce`] is
//! the answer, and it is per trigger rather than global so two operators
//! hitting two buttons at once are not mistaken for one bouncing.
//!
//! # The mapping lives on the node's disk, not on the server
//!
//! It has to. A mapping fetched from the server is a mapping that is missing on
//! the morning the server is down, which is the one morning these surfaces are
//! for. `<state-dir>/surfaces.json`, read at boot.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use crate::link::FromLink;

/// How close together two edges must be to count as one action.
///
/// Relay bounce is typically under 5 ms and a human cannot press a button twice
/// in 120 ms. Wide enough to swallow the first, narrow enough not to swallow a
/// deliberate double-tap on a Stream Deck.
pub const DEBOUNCE: Duration = Duration::from_millis(120);

/// What one trigger does.
///
/// Deliberately the same op vocabulary as every other surface — a trigger is a
/// cue with the arguments filled in ahead of time, not a new kind of thing.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct Trigger {
    pub op: String,
    #[serde(default)]
    pub item: Option<String>,
    #[serde(default)]
    pub output: Option<String>,
    #[serde(default)]
    pub layer: Option<String>,
}

/// The node's surface mapping, as read from disk.
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct Mapping {
    /// OSC address pattern → trigger, e.g. `"/pmg/take/lower-third"`.
    #[serde(default)]
    pub osc: HashMap<String, Trigger>,
    /// MIDI, keyed `"note:60"` or `"cc:7"`.
    #[serde(default)]
    pub midi: HashMap<String, Trigger>,
    /// GPI, keyed by pin number as a string. Also what RossTalk `GPI <n>` hits.
    #[serde(default)]
    pub gpi: HashMap<String, Trigger>,
    /// RossTalk `CC <slot>`, keyed by slot. For switchers that speak only
    /// custom controls.
    #[serde(default)]
    pub cc: HashMap<String, Trigger>,
}

impl Mapping {
    /// Read the mapping, or an empty one if the file is absent.
    ///
    /// Absent is NORMAL — most nodes have no GPI box — so it is not an error.
    /// A file that exists and does not parse **is**: a typo in a mapping must
    /// not silently leave a footswitch inert, because the way that is
    /// discovered is by pressing it during a service.
    pub fn load(dir: &std::path::Path) -> Result<Self> {
        let path = dir.join("surfaces.json");
        if !path.exists() {
            return Ok(Self::default());
        }
        let bytes = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
        let m: Mapping = serde_json::from_slice(&bytes)
            .with_context(|| format!("{} is not a valid surface mapping", path.display()))?;
        tracing::info!(
            osc = m.osc.len(),
            midi = m.midi.len(),
            gpi = m.gpi.len(),
            "surface mapping loaded — these work with the server down"
        );
        Ok(m)
    }
}

/// Per-trigger bounce suppression.
///
/// Keyed by trigger rather than global, so two operators pressing two buttons
/// in the same millisecond are two actions and one relay chattering is one.
#[derive(Default)]
pub struct Debounce(HashMap<String, Instant>);

impl Debounce {
    /// True if this trigger should fire now.
    pub fn accept(&mut self, key: &str) -> bool {
        let now = Instant::now();
        match self.0.get(key) {
            Some(last) if now.duration_since(*last) < DEBOUNCE => false,
            _ => {
                self.0.insert(key.to_string(), now);
                true
            }
        }
    }
}

/// Build the cue a trigger stands for.
pub fn cue_for(t: &Trigger, surface: &str, seq: u64) -> Option<pmg_proto::Cue> {
    let output = t.output.clone().unwrap_or_else(|| "out-0".into());
    let layer = t.layer.clone().unwrap_or_else(|| "Lower Third".into());
    let op = match t.op.as_str() {
        "take" => pmg_proto::CueOp::Take {
            item: t.item.clone()?,
            output,
            layer,
            destinations: Vec::new(),
        },
        "take_out" => pmg_proto::CueOp::TakeOut { output, layer },
        "clear_layer" => pmg_proto::CueOp::ClearLayer { output, layer },
        "clear_all" => pmg_proto::CueOp::ClearAll { output },
        "arm" => pmg_proto::CueOp::Arm {
            item: t.item.clone()?,
            output,
            layer,
        },
        _ => return None,
    };
    Some(pmg_proto::Cue {
        // Minted here, because here is where the human acted. The counter makes
        // successive presses distinct; the debounce above is what makes one
        // press one action.
        cue_id: format!("{surface}-{seq}"),
        // 0 — not part of the server's ordering. See `engine::apply`.
        seq: 0,
        epoch: 0,
        deadline_ms: Some(pmg_proto::DEFAULT_TAKE_DEADLINE_MS),
        at: None,
        op,
    })
}

/// Submit a trigger's cue on the same channel every other surface uses.
pub fn fire(tx: &Sender<FromLink>, t: &Trigger, surface: &str, seq: u64) {
    let Some(cue) = cue_for(t, surface, seq) else {
        tracing::warn!(op = %t.op, surface, "mapping names an op this surface cannot send");
        return;
    };
    tracing::info!(surface, cue = %cue.cue_id, op = %t.op, "surface fired");
    // No reply channel: a contact closure has nowhere to show an ack. The
    // result is in the node's log and in the cue log once the server is back.
    let _ = tx.send(FromLink::Cue(Box::new(cue), Instant::now(), None));
}

// ---------------------------------------------------------------------------
// OSC
// ---------------------------------------------------------------------------

/// Read the address out of an OSC packet.
///
/// Deliberately partial. OSC 1.0 messages are an address string, a type-tag
/// string, then arguments — each null-terminated and padded to a 4-byte
/// boundary. **Only the address is read**, because a trigger is a button: what
/// it does lives in the mapping, not in the arguments, and parsing floats
/// nobody sends would be building for nobody.
///
/// A bundle (`#bundle`) is refused rather than half-handled. Bundles carry a
/// timetag, which is a scheduling promise this node does not make — the same
/// reason `Cue.at` is rejected rather than ignored (pmg-link.md §5.5).
pub fn osc_address(packet: &[u8]) -> Option<String> {
    if packet.first() != Some(&b'/') {
        return None;
    }
    let end = packet.iter().position(|b| *b == 0)?;
    let addr = std::str::from_utf8(&packet[..end]).ok()?;
    // Printable ASCII only. A mapping key is written by a human into a JSON
    // file, so anything else is a malformed packet rather than an address.
    if !addr.bytes().all(|b| (0x20..0x7f).contains(&b)) {
        return None;
    }
    Some(addr.to_string())
}

/// Listen for OSC over UDP forever. Runs on its own thread.
///
/// UDP because that is what OSC is, and because a lighting desk or a QLab
/// machine sending a cue must not be able to block on this node accepting a
/// connection.
pub fn serve_osc(port: u16, mapping: Mapping, tx: Sender<FromLink>) -> Result<()> {
    if mapping.osc.is_empty() {
        tracing::debug!("no OSC triggers mapped; not listening");
        return Ok(());
    }
    let sock = std::net::UdpSocket::bind(("0.0.0.0", port))?;
    tracing::info!(port, triggers = mapping.osc.len(), "OSC listening");

    let mut debounce = Debounce::default();
    let mut seq = 0u64;
    // 4 KB: an OSC message carrying an address and a few arguments is tens of
    // bytes, and anything that does not fit is not a trigger.
    let mut buf = [0u8; 4096];
    loop {
        let Ok((n, from)) = sock.recv_from(&mut buf) else {
            continue;
        };
        let Some(addr) = osc_address(&buf[..n]) else {
            tracing::debug!(%from, "unparseable OSC packet ignored");
            continue;
        };
        let Some(trigger) = mapping.osc.get(&addr) else {
            // Named, at debug: a desk sending addresses this node does not map
            // is normal, and at warn it would fill the log during a service.
            tracing::debug!(%addr, %from, "no trigger mapped for this OSC address");
            continue;
        };
        if !debounce.accept(&addr) {
            continue;
        }
        seq += 1;
        fire(&tx, trigger, "osc", seq);
    }
}

// ---------------------------------------------------------------------------
// RossTalk
// ---------------------------------------------------------------------------

/// Turn one RossTalk line into a trigger.
///
/// `docs/specs/control-surfaces.md` §5. Three forms:
///
/// ```text
/// GPI <n>                                   a configured slot
/// CC <slot>                                 a configured slot
/// CUSTOM take|<output>|<layer>|<item>       self-describing
/// CUSTOM takeout|<output>|<layer>
/// CUSTOM clear|<output>|<layer>
/// CUSTOM clearall|<output>
/// ```
///
/// `CUSTOM` is the real surface. `GPI` and `CC` exist for switchers that can
/// only send those, and they resolve through the same mapping file the physical
/// GPI inputs use — one place where a slot number means something, rather than
/// two that can disagree.
pub fn rosstalk_trigger(line: &str, mapping: &Mapping) -> Option<Trigger> {
    let line = line.trim();
    // Case-insensitive on the verb: switchers differ, and a panel that shouts
    // is not a panel that is wrong.
    let (verb, rest) = line.split_once(char::is_whitespace)?;
    let rest = rest.trim();
    match verb.to_ascii_uppercase().as_str() {
        "GPI" => mapping.gpi.get(rest).cloned(),
        "CC" => mapping.cc.get(rest).cloned(),
        "CUSTOM" => {
            let mut f = rest.split('|').map(str::trim);
            let op = f.next()?.to_ascii_lowercase();
            let (a, b, c) = (f.next(), f.next(), f.next());
            Some(match op.as_str() {
                "take" => Trigger {
                    op: "take".into(),
                    output: a.map(Into::into),
                    layer: b.map(Into::into),
                    item: Some(c?.into()),
                },
                "takeout" => Trigger {
                    op: "take_out".into(),
                    output: a.map(Into::into),
                    layer: b.map(Into::into),
                    item: None,
                },
                "clear" => Trigger {
                    op: "clear_layer".into(),
                    output: a.map(Into::into),
                    layer: b.map(Into::into),
                    item: None,
                },
                "clearall" => Trigger {
                    op: "clear_all".into(),
                    output: a.map(Into::into),
                    layer: None,
                    item: None,
                },
                _ => return None,
            })
        }
        _ => None,
    }
}

/// Listen for RossTalk over TCP forever. Runs on its own thread.
pub fn serve_rosstalk(port: u16, mapping: Mapping, tx: Sender<FromLink>) -> Result<()> {
    let listener = std::net::TcpListener::bind(("0.0.0.0", port))?;
    tracing::info!(port, "RossTalk listening");
    let mut conn_id = 0u64;
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        conn_id += 1;
        let (mapping, tx) = (mapping.clone(), tx.clone());
        std::thread::spawn(move || rosstalk_conn(stream, conn_id, &mapping, &tx));
    }
    Ok(())
}

fn rosstalk_conn(stream: std::net::TcpStream, conn: u64, mapping: &Mapping, tx: &Sender<FromLink>) {
    use std::io::BufRead;
    let peer = stream
        .peer_addr()
        .map(|a| a.to_string())
        .unwrap_or_else(|_| "?".into());
    tracing::info!(%peer, conn, "RossTalk connected");

    let reader = std::io::BufReader::new(stream);
    let mut debounce = Debounce::default();
    let mut seq = 0u64;
    for line in reader.lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let Some(trigger) = rosstalk_trigger(&line, mapping) else {
            // Named at info, not warn: a switcher's macro sending commands this
            // node does not map is a configuration question, and the operator
            // asking "did it arrive?" needs to see that it did.
            tracing::info!(%peer, line = %line.trim(), "RossTalk line not mapped");
            continue;
        };
        // Debounced on the LINE, so a switcher that sends the same command
        // twice in a macro fires once. The per-connection counter below makes
        // successive DIFFERENT commands distinct; it cannot dedupe a genuine
        // retry, because a retried line carries no sequence of its own — that
        // would need the client to number them, and RossTalk has no field for
        // it.
        if !debounce.accept(line.trim()) {
            continue;
        }
        seq += 1;
        fire(tx, &trigger, &format!("rosstalk-{conn}"), seq);
    }
    tracing::info!(%peer, conn, "RossTalk disconnected");
}
