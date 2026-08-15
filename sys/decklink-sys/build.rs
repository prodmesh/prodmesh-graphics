//! Build script for decklink-sys.
//!
//! The DeckLink SDK is licensed separately by Blackmagic and is NOT vendored.
//! A developer who needs it drops it in `vendor/` (gitignored). Without it this
//! crate builds as stubs, so `cargo build --workspace` succeeds on a machine
//! that has never seen the SDK — CI runners included. That is a supported
//! configuration, not a degraded one.

use std::path::Path;

fn main() {
    // Declared so `available()` can test it without an unexpected-cfg warning.
    println!("cargo:rustc-check-cfg=cfg(decklink_sdk_present)");
    println!("cargo:rerun-if-changed=shim/pmg_decklink.h");
    println!("cargo:rerun-if-changed=shim/pmg_decklink.cpp");
    println!("cargo:rerun-if-env-changed=DECKLINK_SDK_DIR");

    let sdk = std::env::var("DECKLINK_SDK_DIR")
        .ok()
        .filter(|p| Path::new(p).exists())
        .or_else(|| {
            let v = Path::new(env!("CARGO_MANIFEST_DIR")).join("vendor");
            v.exists().then(|| v.display().to_string())
        });

    match sdk {
        Some(dir) => {
            // Spike S3 implements shim/pmg_decklink.cpp and turns this on:
            //
            //   cc::Build::new()
            //       .cpp(true)
            //       .include(&dir)
            //       .include("shim")
            //       .file("shim/pmg_decklink.cpp")
            //       .compile("pmg_decklink");
            //
            // Until then, having the SDK present changes nothing.
            println!("cargo:warning=DeckLink SDK found at {dir}, but the shim is not implemented yet (spike S3)");
            println!("cargo:rustc-cfg=decklink_sdk_present");
        }
        None => {
            // Normal for anyone not working on the DeckLink backend. Silent on
            // purpose — a warning on every build trains people to ignore them.
        }
    }
}
