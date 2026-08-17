//! PMG Link message types — the wire contract of record.
//!
//! Spec: `docs/specs/pmg-link.md`. Decision: ADR 0007.
//!
//! One definition serving both ends: the Rust node and, by matching these tags
//! exactly, the plain-ESM Express server. CBOR on the wire.
//!
//! # The framing decision that matters most
//!
//! **Three connections, not one.** Control on this WebSocket, assets over plain
//! HTTP, preview on its own socket. A 4 GB video must not head-of-line-block a
//! take, and a preview stall must not delay a cue. That split *is* the
//! mitigation for most of risk R8 — it is structural rather than a scheduling
//! heuristic, and spike S8 verifies it has not been lost in a refactor.
//!
//! # What `seq` and acks are actually for
//!
//! TCP already provides ordered reliable delivery. This machinery earns its
//! keep **across reconnects** and for idempotency — which is exactly where a
//! naive design loses cues, and exactly what nobody tests. Do not add machinery
//! for reordering.
//!
//! # Why the enums are externally tagged with explicit names
//!
//! Every message and op carries its name as a CBOR map key (`{"Hello": {…}}`).
//! That costs a few bytes per frame and buys something worth more: the
//! JavaScript side reads the same tag, and a frame captured off the wire is
//! legible without a schema. This protocol is debugged in a booth at 09:50 on a
//! Sunday, and self-describing frames are how that goes quickly.

use serde::{Deserialize, Serialize};

/// Deserializers that accept an integer where a float is declared.
///
/// **JavaScript has one number type.** A field the schema calls `f64` arrives
/// as a CBOR integer whenever its value happens to be whole — a score of `3`, a
/// frame rate of exactly `60` — and serde refuses that by default with
/// "invalid type: integer `3`, expected float". The refusal is correct for a
/// Rust-to-Rust format and wrong for a wire shared with a language that cannot
/// express the distinction.
///
/// So every float on this wire is lenient, and every **integer** stays strict:
/// a `u64` that arrived as a float would mean something upstream lost precision
/// and should be loud (see `server/wire.js`, which had exactly that bug).
mod lenient {
    use serde::{Deserialize, Deserializer};

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum NumberOrInt {
        Float(f64),
        Signed(i64),
        Unsigned(u64),
    }

    fn any(v: NumberOrInt) -> f64 {
        match v {
            NumberOrInt::Float(f) => f,
            NumberOrInt::Signed(i) => i as f64,
            NumberOrInt::Unsigned(u) => u as f64,
        }
    }

    pub fn f64<'de, D: Deserializer<'de>>(d: D) -> Result<f64, D::Error> {
        NumberOrInt::deserialize(d).map(any)
    }

    pub fn f32<'de, D: Deserializer<'de>>(d: D) -> Result<f32, D::Error> {
        NumberOrInt::deserialize(d).map(|v| any(v) as f32)
    }
}

/// Protocol version, negotiated at handshake.
pub const PROTO_VER: u32 = 1;

/// Application-level liveness. A TCP connection over failing wifi can take 30+
/// seconds to report an error, and the UI must grey a node's layers within
/// seconds.
pub const PING_INTERVAL_MS: u64 = 1_000;
pub const DEAD_AFTER_MS: u64 = 3_000;

/// Cues the server can replay to a reconnecting node before falling back to a
/// full resync.
pub const REPLAY_RING: usize = 1024;

/// How long a live rebind takes to morph from the old layout to the new one.
///
/// 400 ms reads as the graphic responding rather than as a cut or as a wait.
/// The value is on the wire rather than compiled in because a verse moving to
/// its next section and a typo being corrected want different answers.
pub const DEFAULT_TWEEN_MS: u32 = 400;

/// Default deadline for a take.
///
/// **A take whose deadline has passed is rejected, not fired late.** A graphic
/// appearing four seconds after the button because the link hiccuped is worse
/// than one that never appears.
pub const DEFAULT_TAKE_DEADLINE_MS: u64 = 500;

/// The scene-format major this node can read. A template needing more is
/// refused at **assignment** time, not at take time.
pub const SCENE_READER_MAJOR: u32 = 1;

// ---------------------------------------------------------------------------
// Envelope
// ---------------------------------------------------------------------------

/// Node → server.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ToServer {
    Hello(Hello),
    AssignmentNeed {
        gen: u64,
    },
    AssignmentOk {
        gen: u64,
    },
    AssignmentAck {
        gen: u64,
    },
    /// The node's view of what is on air. **The server adopts this; it never
    /// overwrites it** (ADR 0009).
    StateReport(StateReport),
    SyncProgress(SyncProgress),
    Ready(Ready),
    CueAck(CueAck),
    /// A hole in `seq`. The server replays from its ring or orders a resync.
    CueGap {
        from: u64,
        to: u64,
    },
    Pong {
        t_ms: u64,
    },
}

/// Server → node.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ToNode {
    Welcome(Welcome),
    AssignmentOffer {
        gen: u64,
        digest: String,
    },
    Assignment(Assignment),
    Cue(Cue),
    /// The gap was larger than the replay ring. Start over from the handshake.
    Resync {
        reason: String,
    },
    Ping {
        t_ms: u64,
    },
}

// ---------------------------------------------------------------------------
// Handshake
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Hello {
    pub proto_ver: u32,
    /// Stable, generated once, stored on disk.
    pub node_id: String,
    /// **New every process start.** Together with `epoch` this is what
    /// distinguishes "the node restarted" from "the server restarted" — and
    /// those two need opposite responses (§2.2).
    pub boot_id: String,
    pub name: String,
    pub version: String,
    pub assignment_gen: Option<u64>,
    pub last_applied_seq: u64,
    pub capabilities: Capabilities,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Welcome {
    pub server_id: String,
    /// **New every server start.** Cue idempotency keys are scoped to it, so a
    /// restarted server cannot collide with ids the node already applied.
    pub epoch: u64,
    /// `Some(n)` → replay from n. `None` → full resync.
    pub resume_from_seq: Option<u64>,
    pub time_ref: TimeRef,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct TimeRef {
    pub utc_ms: u64,
    pub monotonic_ns: u64,
}

/// **Discovered, never configured.** What the hardware actually reported.
///
/// A capability the operator typed in is a capability that is wrong the day the
/// card is moved — and a DeckLink read through the wrong SDK version reports
/// defaults rather than errors (see `docs/INTEGRATION-NOTES.md`), so the honest
/// source is the query, every boot.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Capabilities {
    pub outputs: Vec<OutputCapability>,
    pub gpu: Option<GpuCapability>,
    pub decoders: Vec<String>,
    pub max_layers: u32,
    pub scene_reader_major: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct OutputCapability {
    pub device: String,
    /// `decklink` | `ndi` | `window`
    pub kind: String,
    pub index: u32,
    pub modes: Vec<String>,
    pub supports_key_fill: bool,
    pub paired_port: Option<u32>,
    pub has_genlock: bool,
    pub genlock_locked: bool,
    pub ref_format: Option<String>,
    pub audio_pairs: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct GpuCapability {
    pub adapter: String,
    pub backend: String,
    pub max_texture_2d: u32,
}

// ---------------------------------------------------------------------------
// Assignment
// ---------------------------------------------------------------------------

/// What a node is being told it must be able to play.
///
/// Not what it IS playing. Nothing here says `on_air`, and nothing ever should:
/// that is the node's own answer, reported in [`StateReport`].
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Assignment {
    pub gen: u64,
    pub digest: String,
    pub outputs: Vec<OutputAssignment>,
    pub templates: Vec<TemplateRef>,
    pub items: Vec<ItemRef>,
    /// Every hash the node must hold before any of this is armable.
    pub assets: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct OutputAssignment {
    pub id: String,
    pub mode: String,
    /// Colour handling for this output. Defaults are 709 narrow, which is what
    /// an HD plant is — but a default is not an assumption anyone should have
    /// to guess at, so it travels explicitly.
    #[serde(default)]
    pub color: ColorConfig,
    /// **Addressed by name, never by index.** An index silently re-points at a
    /// different graphic when someone reorders the stack.
    pub layers: Vec<String>,
}

/// Per-output colour, range, and key handling.
///
/// **`key_range` is separate from `range` on purpose.** A plant with a
/// narrow-range fill and a full-range key is a real configuration, and folding
/// them into one setting makes it unrepresentable
/// (`docs/specs/color-and-key.md` §4).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ColorConfig {
    /// `bt601` | `bt709` | `bt2020`
    pub matrix: String,
    /// `narrow` | `full`
    pub range: String,
    /// `narrow` | `full`
    pub key_range: String,
    /// `fill` | `key` | `fill_key` | `embedded`
    pub key_mode: String,
}

impl Default for ColorConfig {
    fn default() -> Self {
        Self {
            matrix: "bt709".into(),
            range: "narrow".into(),
            key_range: "narrow".into(),
            key_mode: "fill".into(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct TemplateRef {
    /// `blake3:…` — also the CAS address of the SceneDoc, because the revision
    /// id IS the hash of its bytes.
    pub template_rev: String,
    pub name: String,
    /// The node refuses this template **here**, at assignment, if it exceeds
    /// its own reader major. You find out Saturday, not at 10:15 Sunday.
    pub min_reader: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ItemRef {
    pub id: String,
    pub label: String,
    pub template_rev: String,
    /// Field values, already validated by the server against the pinned
    /// revision's schema. CBOR-native so the node needs no JSON parser.
    pub data: std::collections::BTreeMap<String, DataValue>,
    pub output: Option<String>,
    pub layer: Option<String>,
}

/// A field value. Deliberately a closed set — the same closed set the scene
/// format's `fields` declare, so an Item cannot smuggle a type the renderer has
/// no case for.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum DataValue {
    Text(String),
    Number(#[serde(deserialize_with = "lenient::f64")] f64),
    Bool(bool),
    /// A content address for an image or video.
    Asset(String),
}

impl DataValue {
    /// The string a text binding sees. Non-text values render through their
    /// natural form rather than erroring: bindings are **total** by
    /// construction, and a missing line is worse than an ugly one.
    pub fn as_binding_str(&self) -> String {
        match self {
            DataValue::Text(s) => s.clone(),
            DataValue::Number(n) => {
                // Integral values print without a trailing `.0`: a score of 3
                // is "3", not "3.0".
                if n.fract() == 0.0 && n.is_finite() {
                    format!("{}", *n as i64)
                } else {
                    format!("{n}")
                }
            }
            DataValue::Bool(b) => b.to_string(),
            DataValue::Asset(h) => h.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// Staging
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SyncProgress {
    pub total_bytes: u64,
    pub done_bytes: u64,
    pub pending: u32,
    pub current: Option<String>,
    pub errors: Vec<String>,
}

/// **An Item is not armable until this says it is ready.** Not "warns" — not
/// armable. The Show view greys the button and names what is outstanding.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Ready {
    pub assignment_gen: u64,
    pub items_ready: Vec<String>,
    pub items_blocked: Vec<BlockedItem>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct BlockedItem {
    pub item: String,
    pub missing: Vec<String>,
    /// Set when the block is a version refusal rather than a missing byte, with
    /// both numbers in it so the message can name them.
    pub unreadable: Option<String>,
}

// ---------------------------------------------------------------------------
// Cues
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Cue {
    /// **Generated where the human acted** and carried unchanged end to end.
    /// That is what makes a double-click, a bouncing GPI contact, or an HTTP
    /// retry dedupe at the node. A client that mints a fresh id per retry has
    /// defeated the mechanism.
    pub cue_id: String,
    pub seq: u64,
    pub epoch: u64,
    /// `None` for `Arm` and `Preload`, which have nothing to be late for.
    pub deadline_ms: Option<u64>,
    /// Reserved. **Always `None` in v1**, and a v1 node must reject a non-null
    /// `at` with `unsupported` rather than ignore it. The field exists now
    /// because adding it later is a protocol break.
    pub at: Option<CueAt>,
    pub op: CueOp,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum CueAt {
    Frame(u64),
    Timecode(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum CueOp {
    Arm {
        item: String,
        output: String,
        layer: String,
    },
    Take {
        item: String,
        output: String,
        layer: String,
    },
    TakeOut {
        output: String,
        layer: String,
    },
    Update {
        output: String,
        layer: String,
        data: std::collections::BTreeMap<String, DataValue>,
        /// How long the rebind takes to morph, in milliseconds.
        ///
        /// `None` means [`DEFAULT_TWEEN_MS`]. `Some(0)` is a hard cut, which is
        /// what a correction wants — fixing a misspelled name should not be a
        /// 400 ms animation drawing attention to itself.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tween_ms: Option<u32>,
    },
    ClearLayer {
        output: String,
        layer: String,
    },
    ClearAll {
        output: String,
    },
    Timeline {
        output: String,
        layer: String,
        name: String,
        action: TimelineAction,
    },
    Preload {
        item: String,
    },
    /// Show a test pattern on an output, or clear it with `None`.
    ///
    /// A configuration you cannot PROVE is a configuration you are trusting,
    /// and these are the instruments that prove it
    /// (`docs/specs/color-and-key.md` §7).
    TestPattern {
        output: String,
        pattern: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum TimelineAction {
    Play,
    Stop,
    Goto { t_ms: u64 },
}

impl CueOp {
    /// Which output this op acts on, for routing to the right render thread.
    pub fn output(&self) -> Option<&str> {
        match self {
            CueOp::Arm { output, .. }
            | CueOp::Take { output, .. }
            | CueOp::TakeOut { output, .. }
            | CueOp::Update { output, .. }
            | CueOp::ClearLayer { output, .. }
            | CueOp::ClearAll { output }
            | CueOp::Timeline { output, .. }
            | CueOp::TestPattern { output, .. } => Some(output),
            CueOp::Preload { .. } => None,
        }
    }

    /// Ops that put something on air, or take it off, are the ones a deadline
    /// protects. Arming and preloading are invisible, so being late costs
    /// nothing and rejecting them would be pointless strictness.
    pub fn is_deadline_bearing(&self) -> bool {
        !matches!(self, CueOp::Arm { .. } | CueOp::Preload { .. })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CueAck {
    pub seq: u64,
    pub cue_id: String,
    pub result: CueResult,
    pub reason: Option<RejectReason>,
    pub fired_at_frame: Option<u64>,
    pub at_utc: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum CueResult {
    Applied,
    /// Already applied. Not an error — this is the mechanism working.
    Deduped,
    Rejected,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum RejectReason {
    DeadlinePassed,
    NotReady,
    NoSuchLayer,
    NoSuchItem,
    UnreadableTemplate,
    /// A v1 node saw a non-null `at`. Refusing beats silently ignoring it: a
    /// scheduled take that fires immediately is worse than one that does not
    /// fire, because nobody finds out.
    Unsupported,
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct StateReport {
    pub gen: u64,
    pub last_applied_seq: u64,
    pub outputs: Vec<OutputState>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct OutputState {
    pub id: String,
    /// Reference/genlock, as the hardware reports it. `None` for a backend with
    /// no such concept (NDI, headless) — which is different from "present:
    /// false", and the UI must not conflate them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sync: Option<SyncState>,
    pub layers: Vec<LayerState>,
    #[serde(deserialize_with = "lenient::f32")]
    pub fps: f32,
    pub late_frames: u64,
}

/// What the card says about its reference input.
///
/// **Genlock gives phase, not frame numbering** (ADR 0011). Two nodes on one
/// reference output in lockstep and still disagree about which frame is number
/// 1841944, so `locked` here promises phase within this node and nothing about
/// simultaneity across nodes.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SyncState {
    pub present: bool,
    pub locked: bool,
    pub format: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct LayerState {
    pub name: String,
    pub occupant: Option<Occupant>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Occupant {
    pub item_id: String,
    /// The words the operator recognises. Carried on the wire rather than
    /// looked up server-side, because the node is the authority on what is on
    /// air and a server that just restarted has no other way to caption it.
    pub item_label: String,
    pub template_rev: String,
    /// `in` | `hold` | `out`
    pub phase: String,
    pub timeline: Option<String>,
    pub t_ms: u64,
    /// Set when the scene rendered with a magenta placeholder or an unknown
    /// transform treated as identity. It is on air and it is wrong, and saying
    /// so is the whole point.
    pub degraded: bool,
}

// ---------------------------------------------------------------------------
// Framing
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum ProtoError {
    #[error("encode: {0}")]
    Encode(String),
    #[error("decode: {0}")]
    Decode(String),
}

pub fn encode<T: Serialize>(msg: &T) -> Result<Vec<u8>, ProtoError> {
    let mut out = Vec::new();
    ciborium::into_writer(msg, &mut out).map_err(|e| ProtoError::Encode(e.to_string()))?;
    Ok(out)
}

pub fn decode<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<T, ProtoError> {
    ciborium::from_reader(bytes).map_err(|e| ProtoError::Decode(e.to_string()))
}

pub fn selftest() -> Result<(), String> {
    // The round trip is the contract. If this breaks, every node stops talking
    // to every server, so it is worth a selftest rather than only a unit test.
    let cue = Cue {
        cue_id: "01JA".into(),
        seq: 8813,
        epoch: 19,
        deadline_ms: Some(DEFAULT_TAKE_DEADLINE_MS),
        at: None,
        op: CueOp::Take {
            item: "01JB".into(),
            output: "out-0".into(),
            layer: "Lower Third".into(),
        },
    };
    let bytes = encode(&ToNode::Cue(cue.clone())).map_err(|e| e.to_string())?;
    match decode::<ToNode>(&bytes).map_err(|e| e.to_string())? {
        ToNode::Cue(back) if back == cue => Ok(()),
        other => Err(format!("cue did not round-trip: {other:?}")),
    }
}
