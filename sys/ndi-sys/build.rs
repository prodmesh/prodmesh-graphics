//! Build script for ndi-sys.
//!
//! The NDI runtime is **loaded at runtime, never linked**, so redistribution
//! stays a deployment question rather than a build-time commitment
//! (ADR 0003 §Seam). There is therefore nothing to do at build time — this file
//! exists to document that, and so nobody adds a link directive later without
//! reading why.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
}
