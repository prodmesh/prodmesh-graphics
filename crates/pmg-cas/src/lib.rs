//! Content-addressed store: hash, put, get, verify, disk cache, GC.
//!
//! Spec: `docs/specs/asset-cas.md`. Decision: ADR 0006.
//!
//! Identical on the server and on every node, so the same verification pass
//! works in both places.
//!
//! # Rules
//!
//! - **Writes are atomic**: stream to `tmp/`, hash while writing, verify, then
//!   `rename(2)` into `blobs/`. A partial file is never visible as complete.
//! - **Every blob is verified against its hash after download.** Three
//!   mismatches is a hard error naming the hash — that means something is
//!   corrupting data in transit or at rest, and it should be loud.
//! - **The cache is permanent by default, not LRU.** Disk is cheap; a
//!   re-download on the morning of a service is not. GC is an explicit operator
//!   action that reports before it removes, and **never runs automatically on
//!   low disk** — a node that silently deletes an asset it needs in an hour has
//!   turned a disk warning into an on-air failure.
//!
//! # M0 status
//!
//! Types only. blake3 lands with the implementation in M1.

/// Two-level fan-out on the first two bytes of the hash, so a directory never
/// holds a pathological number of entries.
pub const FANOUT_DEPTH: usize = 2;

pub fn selftest() -> Result<(), String> {
    Ok(())
}
