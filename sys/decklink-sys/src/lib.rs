//! Raw FFI to the DeckLink shim.
//!
//! **The contract of record is `shim/pmg_decklink.h`**, written by hand. The
//! declarations below are transcribed from it by hand. If they disagree, the
//! header is right. Never point bindgen at the DeckLink SDK's C++ headers — see
//! the header's own comment, ADR 0003, and risk R4.
//!
//! This crate is one of exactly two places in the workspace where `unsafe` is
//! permitted. Safe wrappers live in `pmg-out-decklink`.
//!
//! # M0 status
//!
//! Declarations only; `shim/pmg_decklink.cpp` is unimplemented. Spike S3 builds
//! it and reports whether this approach holds at all. Until then
//! [`available()`] answers `false` and nothing links.

#![allow(non_camel_case_types)]

/// Zero is success; negative is failure. See the header for the code list.
pub type pmg_dl_result = i32;

pub const PMG_DL_OK: pmg_dl_result = 0;
pub const PMG_DL_ERR_NO_SDK: pmg_dl_result = -1;
pub const PMG_DL_ERR_NO_DEVICE: pmg_dl_result = -2;
pub const PMG_DL_ERR_BAD_MODE: pmg_dl_result = -3;
pub const PMG_DL_ERR_NO_KEY_FILL: pmg_dl_result = -4;
pub const PMG_DL_ERR_BUSY: pmg_dl_result = -5;
pub const PMG_DL_ERR_INTERNAL: pmg_dl_result = -6;

/// Frame completion as reported by the SDK.
///
/// `DisplayedLate` is the value spike S3 counts: its success bar is zero of
/// these over ten minutes at 1080p59.94.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum Completion {
    Completed = 0,
    DisplayedLate = 1,
    Dropped = 2,
    Flushed = 3,
}

/// Whether this build has a real SDK behind it.
///
/// `false` is a supported configuration: `cargo build --workspace` must succeed
/// on a machine that has never seen the DeckLink SDK.
pub const fn available() -> bool {
    cfg!(decklink_sdk_present)
}

// The `extern "C"` block transcribing shim/pmg_decklink.h is added by spike S3,
// alongside the .cpp that implements it. Declaring symbols that nothing defines
// would break `cargo build --workspace` on every machine without the SDK, which
// is precisely the configuration this crate is shaped to support.

#[cfg(test)]
mod tests {
    #[test]
    fn header_is_the_contract_of_record() {
        // If the header ever goes missing, that is a much bigger problem than a
        // failing build — it is the only description of this boundary.
        let h = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("shim/pmg_decklink.h");
        assert!(
            h.exists(),
            "shim/pmg_decklink.h is the contract of record and must exist"
        );
    }
}
