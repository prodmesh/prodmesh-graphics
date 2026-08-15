//! PMG Link message types — the wire contract of record.
//!
//! Spec: `docs/specs/pmg-link.md`. Decision: ADR 0007.
//!
//! One definition serving both ends: the Rust node and (via generated schema)
//! the plain-ESM Express server. CBOR on the wire.
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
//! # M0 status
//!
//! Constants only. Message types land in M1.

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

/// Default deadline for a take.
///
/// **A take whose deadline has passed is rejected, not fired late.** A graphic
/// appearing four seconds after the button because the link hiccuped is worse
/// than one that never appears.
pub const DEFAULT_TAKE_DEADLINE_MS: u64 = 500;

pub fn selftest() -> Result<(), String> {
    Ok(())
}
