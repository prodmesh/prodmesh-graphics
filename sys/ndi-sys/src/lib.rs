//! Raw FFI to the NDI SDK.
//!
//! NDI is a C API, so unlike DeckLink (ADR 0003, risk R4) there is no COM-alike
//! to wrap — but the same rule applies: **the runtime is loaded dynamically,
//! never linked.** That keeps redistribution a deployment question rather than a
//! build-time commitment, and it means a build without the SDK present is a
//! supported configuration.
//!
//! One of exactly two places in the workspace where `unsafe` is permitted. Safe
//! wrappers live in `pmg-out-ndi`.
//!
//! # Alpha
//!
//! NDI wants **straight** alpha. Everything internal is premultiplied, so
//! `pmg-color::unpremultiply` runs at this boundary — getting the direction
//! wrong produces fringing that is invisible over one background and obvious
//! over the other (`docs/specs/color-and-key.md` §5).
//!
//! # M0 status
//!
//! Stub. The runtime loader lands with the M1 output backend.

#![allow(non_camel_case_types)]

/// Library names to try, in order, when loading the runtime.
pub const RUNTIME_CANDIDATES: &[&str] = &[
    "libndi.dylib",
    "libndi.so.5",
    "libndi.so",
    "Processing.NDI.Lib.x64.dll",
];

/// Whether the NDI runtime could be loaded.
///
/// Always `false` in M0. When implemented this attempts each candidate and
/// caches the result — Configure shows it as a plain sentence rather than an
/// error code, per `docs/UI_TEXT.md`.
pub fn runtime_available() -> bool {
    false
}
