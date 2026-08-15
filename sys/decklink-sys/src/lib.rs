//! Raw FFI to the DeckLink shim.
//!
//! **The contract of record is `shim/pmg_decklink.h`**, written by hand. The
//! declarations below are transcribed from it by hand. If they disagree, the
//! header is right. Never point bindgen at the DeckLink SDK's C++ headers —
//! they are COM on Windows and a hand-rolled COM-alike elsewhere, with virtual
//! interfaces you must *implement*. See ADR 0003 and risk R4.
//!
//! One of exactly two places in the workspace where `unsafe` is permitted. Safe
//! wrappers live in `pmg-out-decklink`; [`Device`] below is the thin, checked
//! layer used by `pmgctl decklink`.
//!
//! # Building without the SDK
//!
//! Everything here is gated on `decklink_sdk_present`, set by `build.rs` when
//! it finds the SDK. Without it, [`available`] is `false` and the API returns
//! [`Error::NoSdk`] — so `cargo build --workspace` succeeds on a machine that
//! has never seen the SDK, which every CI runner is.

#![allow(non_camel_case_types)]

use std::ffi::c_char;

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
/// `DisplayedLate` is the value spike S3 counts: its bar is zero over ten
/// minutes at 1080p59.94.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum Completion {
    Completed = 0,
    DisplayedLate = 1,
    Dropped = 2,
    Flushed = 3,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct pmg_dl_device_info {
    pub display_name: [c_char; 128],
    pub model_name: [c_char; 128],
    pub index: i32,
    pub sub_device_count: i32,
    pub sub_device_index: i32,
    /// **Probed, never assumed.** A model number does not predict this.
    pub supports_key_fill: i32,
    pub supports_internal_key: i32,
    pub has_reference_input: i32,
    pub max_audio_channels: i32,
    pub persistent_id: i64,
    pub topological_id: i64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct pmg_dl_mode {
    pub width: i32,
    pub height: i32,
    /// Exact rational, always. `60000/1001`, never a 59.94 float.
    pub fps_num: i32,
    pub fps_den: i32,
    pub interlaced: i32,
    pub name: [c_char; 64],
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct pmg_dl_reference {
    pub present: i32,
    pub locked: i32,
    pub format: [c_char; 64],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct pmg_dl_stats {
    pub scheduled: u64,
    pub completed: u64,
    pub displayed_late: u64,
    pub dropped: u64,
    pub buffered: u64,
}

pub enum pmg_dl_device {}
pub enum pmg_dl_output {}

#[cfg(decklink_sdk_present)]
extern "C" {
    pub fn pmg_dl_available() -> i32;
    pub fn pmg_dl_sdk_version(buf: *mut c_char, cap: i32);
    pub fn pmg_dl_interface_report(buf: *mut c_char, cap: i32);
    pub fn pmg_dl_device_count() -> i32;
    pub fn pmg_dl_device_info_at(index: i32, out: *mut pmg_dl_device_info) -> pmg_dl_result;
    pub fn pmg_dl_device_open(index: i32, out: *mut *mut pmg_dl_device) -> pmg_dl_result;
    pub fn pmg_dl_device_release(dev: *mut pmg_dl_device);
    pub fn pmg_dl_mode_count(dev: *mut pmg_dl_device) -> i32;
    pub fn pmg_dl_mode_at(dev: *mut pmg_dl_device, i: i32, out: *mut pmg_dl_mode) -> pmg_dl_result;
    pub fn pmg_dl_reference_status(
        dev: *mut pmg_dl_device,
        out: *mut pmg_dl_reference,
    ) -> pmg_dl_result;
    pub fn pmg_dl_output_stats(out: *mut pmg_dl_output, stats: *mut pmg_dl_stats) -> pmg_dl_result;
}

/// Whether this build has a real SDK behind it.
///
/// `false` is a supported configuration.
pub const fn available() -> bool {
    cfg!(decklink_sdk_present)
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(
        "built without the DeckLink SDK. Set DECKLINK_SDK_DIR to the extracted SDK \
         (the directory containing Mac/include/DeckLinkAPI.h) and rebuild"
    )]
    NoSdk,
    #[error("no DeckLink device at index {0}")]
    NoDevice(i32),
    #[error(
        "the DeckLink driver is not loaded. Install Blackmagic Desktop Video, or check \
         that its system extension is enabled"
    )]
    NoDriver,
    #[error("DeckLink call failed with code {0}")]
    Failed(pmg_dl_result),
}

// Only reachable when the SDK is compiled in; without it nothing returns a
// C string to decode.
#[cfg(decklink_sdk_present)]
fn cstr(buf: &[c_char]) -> String {
    let bytes: Vec<u8> = buf
        .iter()
        .take_while(|c| **c != 0)
        .map(|c| *c as u8)
        .collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// SDK version the shim was compiled against.
pub fn sdk_version() -> Option<String> {
    #[cfg(decklink_sdk_present)]
    {
        let mut buf = [0 as c_char; 64];
        // SAFETY: buf is 64 bytes and cap says so; the shim snprintf()s into it.
        unsafe { pmg_dl_sdk_version(buf.as_mut_ptr(), buf.len() as i32) };
        Some(cstr(&buf))
    }
    #[cfg(not(decklink_sdk_present))]
    None
}

/// Which IID generation the driver actually satisfied, per interface.
///
/// **Call this before believing a capability "no".** When the driver is older
/// than the SDK, `QueryInterface` returns `E_NOINTERFACE` and every attribute
/// silently reads as its default — a DeckLink Quad 2 then reports no keying and
/// no audio channels, which is indistinguishable from a card that cannot do
/// anything. Measured on Desktop Video 14.5 against SDK 16.0; see
/// `docs/INTEGRATION-NOTES.md`.
pub fn interface_report() -> Option<String> {
    #[cfg(decklink_sdk_present)]
    {
        // Populate the shim's statics by probing device 0 across all three
        // interfaces — attributes and status via device_info, output via
        // modes(). An interface that is merely unprobed must not read as a
        // failure.
        let _ = device_info(0);
        if let Ok(d) = Device::open(0) {
            let _ = d.modes();
        }
        let mut buf = [0 as c_char; 128];
        // SAFETY: buf is 128 bytes and cap says so; the shim snprintf()s into it.
        unsafe { pmg_dl_interface_report(buf.as_mut_ptr(), buf.len() as i32) };
        Some(cstr(&buf))
    }
    #[cfg(not(decklink_sdk_present))]
    None
}

/// Whether the driver loaded *and* an iterator could be created.
///
/// Having Desktop Video installed is not the same as it working, so this asks
/// the framework rather than checking for a file.
pub fn driver_loaded() -> bool {
    #[cfg(decklink_sdk_present)]
    // SAFETY: no arguments; the shim creates and releases an iterator.
    unsafe {
        pmg_dl_available() != 0
    }
    #[cfg(not(decklink_sdk_present))]
    false
}

/// Number of DeckLink devices present.
pub fn device_count() -> Result<i32, Error> {
    if !available() {
        return Err(Error::NoSdk);
    }
    if !driver_loaded() {
        return Err(Error::NoDriver);
    }
    #[cfg(decklink_sdk_present)]
    // SAFETY: no arguments.
    unsafe {
        Ok(pmg_dl_device_count())
    }
    #[cfg(not(decklink_sdk_present))]
    Err(Error::NoSdk)
}

/// A device's probed capabilities.
#[derive(Debug, Clone)]
pub struct DeviceInfo {
    pub display_name: String,
    pub model_name: String,
    pub index: i32,
    pub sub_device_count: i32,
    pub sub_device_index: i32,
    pub supports_key_fill: bool,
    pub supports_internal_key: bool,
    pub has_reference_input: bool,
    pub max_audio_channels: i32,
    /// Stable across reboots, but **per connector** — measured incrementing
    /// 0xC55080A0..A7 across a Quad 2's eight connectors. Do not group on it.
    pub persistent_id: i64,
    /// The physical **card**. Group on this: a fill+key pair must be two
    /// connectors on one card, because one card is one clock (ADR 0011).
    pub topological_id: i64,
}

pub fn device_info(index: i32) -> Result<DeviceInfo, Error> {
    if !available() {
        return Err(Error::NoSdk);
    }
    #[cfg(decklink_sdk_present)]
    {
        let mut raw = std::mem::MaybeUninit::<pmg_dl_device_info>::zeroed();
        // SAFETY: the shim memsets and fills `out`; index is bounds-checked there.
        let rc = unsafe { pmg_dl_device_info_at(index, raw.as_mut_ptr()) };
        if rc == PMG_DL_ERR_NO_DEVICE {
            return Err(Error::NoDevice(index));
        }
        if rc != PMG_DL_OK {
            return Err(Error::Failed(rc));
        }
        // SAFETY: the shim zeroed and populated it on the PMG_DL_OK path.
        let i = unsafe { raw.assume_init() };
        Ok(DeviceInfo {
            display_name: cstr(&i.display_name),
            model_name: cstr(&i.model_name),
            index: i.index,
            sub_device_count: i.sub_device_count,
            sub_device_index: i.sub_device_index,
            supports_key_fill: i.supports_key_fill != 0,
            supports_internal_key: i.supports_internal_key != 0,
            has_reference_input: i.has_reference_input != 0,
            max_audio_channels: i.max_audio_channels,
            persistent_id: i.persistent_id,
            topological_id: i.topological_id,
        })
    }
    #[cfg(not(decklink_sdk_present))]
    {
        let _ = index;
        Err(Error::NoSdk)
    }
}

/// An open device. `Drop` releases it — rule 2 in the header.
pub struct Device {
    #[cfg(decklink_sdk_present)]
    raw: *mut pmg_dl_device,
}

#[derive(Debug, Clone)]
pub struct Mode {
    pub width: i32,
    pub height: i32,
    pub fps_num: i32,
    pub fps_den: i32,
    pub interlaced: bool,
    pub name: String,
}

impl Mode {
    /// The rational rate as a float — for **display only**. Never compute frame
    /// timing from this (`docs/specs/output-config.md` §2).
    pub fn fps(&self) -> f64 {
        self.fps_num as f64 / self.fps_den.max(1) as f64
    }
}

#[derive(Debug, Clone)]
pub struct Reference {
    pub present: bool,
    pub locked: bool,
    pub format: String,
}

impl Device {
    pub fn open(index: i32) -> Result<Self, Error> {
        if !available() {
            return Err(Error::NoSdk);
        }
        #[cfg(decklink_sdk_present)]
        {
            let mut raw: *mut pmg_dl_device = std::ptr::null_mut();
            // SAFETY: out-pointer to a local; the shim writes null on failure.
            let rc = unsafe { pmg_dl_device_open(index, &mut raw) };
            if rc != PMG_DL_OK || raw.is_null() {
                return Err(Error::NoDevice(index));
            }
            Ok(Device { raw })
        }
        #[cfg(not(decklink_sdk_present))]
        {
            let _ = index;
            Err(Error::NoSdk)
        }
    }

    pub fn modes(&self) -> Vec<Mode> {
        #[cfg(decklink_sdk_present)]
        {
            // SAFETY: self.raw is non-null for the lifetime of Device.
            let n = unsafe { pmg_dl_mode_count(self.raw) };
            (0..n)
                .filter_map(|i| {
                    let mut m = std::mem::MaybeUninit::<pmg_dl_mode>::zeroed();
                    // SAFETY: the shim memsets and fills on the OK path.
                    let rc = unsafe { pmg_dl_mode_at(self.raw, i, m.as_mut_ptr()) };
                    (rc == PMG_DL_OK).then(|| {
                        let m = unsafe { m.assume_init() };
                        Mode {
                            width: m.width,
                            height: m.height,
                            fps_num: m.fps_num,
                            fps_den: m.fps_den,
                            interlaced: m.interlaced != 0,
                            name: cstr(&m.name),
                        }
                    })
                })
                .collect()
        }
        #[cfg(not(decklink_sdk_present))]
        Vec::new()
    }

    pub fn reference(&self) -> Option<Reference> {
        #[cfg(decklink_sdk_present)]
        {
            let mut r = std::mem::MaybeUninit::<pmg_dl_reference>::zeroed();
            // SAFETY: the shim memsets and fills on the OK path.
            let rc = unsafe { pmg_dl_reference_status(self.raw, r.as_mut_ptr()) };
            if rc != PMG_DL_OK {
                return None;
            }
            let r = unsafe { r.assume_init() };
            Some(Reference {
                present: r.present != 0,
                locked: r.locked != 0,
                format: cstr(&r.format),
            })
        }
        #[cfg(not(decklink_sdk_present))]
        None
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        #[cfg(decklink_sdk_present)]
        // SAFETY: paired with pmg_dl_device_open; called exactly once.
        unsafe {
            pmg_dl_device_release(self.raw)
        }
    }
}

// Ownership: the shim's enumerate() releases every interface it does not hand
// out, and Device::drop releases the one it does.
//
// Device is deliberately NOT Send or Sync. The DeckLink SDK's documentation is
// inconsistent about which interfaces may cross a thread boundary, and guessing
// here would produce a race that only appears under load — on a Sunday. When the
// output path lands (spike S3), the completion callback's threading is the first
// thing to pin down, and any Send/Sync impl needs a written argument next to it.

#[cfg(test)]
mod tests {
    #[test]
    fn header_is_the_contract_of_record() {
        // If this ever goes missing it is a bigger problem than a failing build:
        // it is the only description of this boundary.
        let h = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("shim/pmg_decklink.h");
        assert!(
            h.exists(),
            "shim/pmg_decklink.h is the contract of record and must exist"
        );
    }

    #[test]
    fn builds_and_answers_without_an_sdk() {
        // The supported no-SDK configuration must not panic — it must report.
        match super::device_count() {
            Ok(n) => assert!(n >= 0),
            Err(e) => {
                let m = e.to_string();
                assert!(!m.is_empty(), "an error must say what to do about it");
            }
        }
    }
}
