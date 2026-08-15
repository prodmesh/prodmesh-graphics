//! Build script for decklink-sys.
//!
//! The DeckLink SDK is licensed separately by Blackmagic and is NOT vendored.
//! Point DECKLINK_SDK_DIR at the extracted SDK, or drop it in vendor/.
//!
//! Without it this crate builds as stubs, so `cargo build --workspace` succeeds
//! on a machine that has never seen the SDK — CI runners included. That is a
//! supported configuration, not a degraded one, and it is why src/lib.rs gates
//! every extern declaration behind `decklink_sdk_present`.
//!
//! Note we link no Blackmagic library: DeckLinkAPIDispatch.cpp dlopens
//! DeckLinkAPI.framework at runtime. The vendor boundary stays at the seam
//! ADR 0003 describes.

use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rustc-check-cfg=cfg(decklink_sdk_present)");
    println!("cargo:rerun-if-changed=shim/pmg_decklink.h");
    println!("cargo:rerun-if-changed=shim/pmg_decklink.cpp");
    println!("cargo:rerun-if-env-changed=DECKLINK_SDK_DIR");

    let Some(inc) = sdk_include_dir() else {
        // Normal for anyone not working on the DeckLink backend. Silent on
        // purpose — a warning on every build trains people to ignore warnings.
        return;
    };

    cc::Build::new()
        .cpp(true)
        .std("c++17")
        .include(&inc)
        .include("shim")
        .file("shim/pmg_decklink.cpp")
        // Provides CreateDeckLinkIteratorInstance(); dlopens the framework.
        .file(inc.join("DeckLinkAPIDispatch.cpp"))
        .flag_if_supported("-Wno-unused-parameter")
        .flag_if_supported("-Wno-multichar")
        .compile("pmg_decklink");

    println!("cargo:rustc-link-lib=framework=CoreFoundation");
    println!("cargo:rustc-cfg=decklink_sdk_present");
}

/// Accept either the SDK root or the platform include dir directly.
fn sdk_include_dir() -> Option<PathBuf> {
    let roots: Vec<PathBuf> = std::env::var("DECKLINK_SDK_DIR")
        .ok()
        .map(PathBuf::from)
        .into_iter()
        .chain(std::iter::once(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("vendor"),
        ))
        .collect();

    for root in roots {
        for candidate in [root.join("Mac/include"), root.join("include"), root.clone()] {
            if candidate.join("DeckLinkAPI.h").exists()
                && candidate.join("DeckLinkAPIDispatch.cpp").exists()
            {
                return Some(candidate);
            }
        }
    }
    None
}
