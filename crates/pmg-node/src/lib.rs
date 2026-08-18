//! The playout node, as a library.
//!
//! The binary is a thin wrapper. The library exists so `pmgctl` can share the
//! arm-and-render path rather than keeping its own copy — the node and the CLI
//! must put identical pixels on the wire, and two implementations of that is
//! two chances to disagree.

pub mod discover;
pub mod engine;
pub mod link;
pub mod local;
pub mod render;
pub mod runtime;
pub mod staging;
pub mod store;
pub mod surface;

pub fn selftest() -> Result<(), String> {
    Ok(())
}
