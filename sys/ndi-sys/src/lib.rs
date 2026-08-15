//! Raw FFI to the NDI runtime, loaded dynamically.
//!
//! **The contract of record is `include/pmg_ndi.h`**, written by hand. The
//! declarations below are transcribed from it. If they disagree, the header is
//! right. See ADR 0003.
//!
//! One of exactly two places in the workspace where `unsafe` is permitted. Safe
//! wrappers live in `pmg-out-ndi`; `Ndi` below is the thin, checked layer.
//!
//! # Why dynamic loading rather than linking
//!
//! Redistribution stays a deployment question rather than a build-time
//! commitment (ADR 0003 §Seam), and `cargo build --workspace` succeeds on a
//! machine that has never seen NDI — which CI runners have not.
//!
//! The runtime exports **flat C symbols**, verified with `nm`, so each `dlsym`
//! is checked individually and a mistake is a named missing symbol rather than a
//! silently wrong offset into the `NDIlib_v5_load()` vtable.
//!
//! # Alpha
//!
//! NDI wants **straight** alpha. Everything internal is premultiplied, so
//! `pmg_color::unpremultiply` runs at this boundary. Getting the direction wrong
//! produces fringing that is invisible over one background and obvious over the
//! other (`docs/specs/color-and-key.md` §5).

#![allow(non_camel_case_types)]
#![allow(non_snake_case)]

use std::ffi::{c_char, c_int, c_void, CStr};

// ---------------------------------------------------------------------------
// Constants. Verified against the SDK headers — see include/pmg_ndi.h.
// ---------------------------------------------------------------------------

/// `NDI_LIB_FOURCC('B','G','R','A')`.
pub const FOURCC_BGRA: c_int = 0x4152_4742;
pub const FRAME_FORMAT_PROGRESSIVE: c_int = 1;
pub const FRAME_TYPE_NONE: c_int = 0;
pub const FRAME_TYPE_VIDEO: c_int = 1;
pub const FRAME_TYPE_AUDIO: c_int = 2;

/// `'FLTP'` — planar 32-bit float, the only audio format NDI sends uncompressed.
pub const FOURCC_AUDIO_FLTP: c_int = i32::from_le_bytes(*b"FLTp");
pub const FRAME_TYPE_ERROR: c_int = 4;
pub const RECV_COLOR_FORMAT_BGRX_BGRA: c_int = 0;
pub const RECV_BANDWIDTH_HIGHEST: c_int = 100;

/// Where to look for the runtime, in order.
///
/// The SDK's own path first, then app bundles that ship it. NDI Tools,
/// Resolume, DashBoard, and ProPresenter all carry a copy, so a machine with any
/// of them installed can run a node without a separate SDK install.
pub const RUNTIME_CANDIDATES: &[&str] = &[
    "/Library/NDI SDK for Apple/lib/macOS/libndi.dylib",
    "/usr/local/lib/libndi.dylib",
    "/Applications/NDI Launcher.app/Contents/Frameworks/libndi.dylib",
    "libndi.dylib",
    "libndi.so.5",
    "libndi.so",
    "Processing.NDI.Lib.x64.dll",
];

// ---------------------------------------------------------------------------
// Structs. Field order and type are load-bearing; the tests below assert every
// offset against numbers emitted by a C probe over the real SDK headers.
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct NDIlib_source_t {
    pub p_ndi_name: *const c_char,
    /// Union with the deprecated `p_ip_address`; same offset.
    pub p_url_address: *const c_char,
}

#[repr(C)]
pub struct NDIlib_find_create_t {
    pub show_local_sources: bool,
    pub p_groups: *const c_char,
    pub p_extra_ips: *const c_char,
}

#[repr(C)]
pub struct NDIlib_recv_create_v3_t {
    pub source_to_connect_to: NDIlib_source_t,
    pub color_format: c_int,
    pub bandwidth: c_int,
    pub allow_video_fields: bool,
    pub p_ndi_recv_name: *const c_char,
}

#[repr(C)]
pub struct NDIlib_send_create_t {
    pub p_ndi_name: *const c_char,
    pub p_groups: *const c_char,
    /// `true` lets the SDK pace us to `frame_rate_N/D`, so a sender free-runs at
    /// the right rate with no timer of our own.
    pub clock_video: bool,
    pub clock_audio: bool,
}

/// The struct this whole exercise is about.
///
/// `#[repr(C)]` places the padding (4 bytes after `frame_format_type`, 4 after
/// `line_stride_in_bytes`) rather than us hard-coding offsets — the tests then
/// confirm it placed them where the C compiler does.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct NDIlib_video_frame_v2_t {
    pub xres: c_int,
    pub yres: c_int,
    pub FourCC: c_int,
    pub frame_rate_N: c_int,
    pub frame_rate_D: c_int,
    pub picture_aspect_ratio: f32,
    pub frame_format_type: c_int,
    pub timecode: i64,
    pub p_data: *mut u8,
    /// Union with `data_size_in_bytes` for compressed FourCCs.
    pub line_stride_in_bytes: c_int,
    pub p_metadata: *const c_char,
    pub timestamp: i64,
}

impl Default for NDIlib_video_frame_v2_t {
    fn default() -> Self {
        Self {
            xres: 0,
            yres: 0,
            FourCC: FOURCC_BGRA,
            frame_rate_N: 30000,
            frame_rate_D: 1001,
            picture_aspect_ratio: 0.0, // 0 = derive from xres/yres
            frame_format_type: FRAME_FORMAT_PROGRESSIVE,
            // NDIlib_send_timecode_synthesize — let the SDK stamp it.
            timecode: i64::MAX,
            p_data: std::ptr::null_mut(),
            line_stride_in_bytes: 0,
            p_metadata: std::ptr::null(),
            timestamp: 0,
        }
    }
}

/// Planar float audio — **what NDI actually carries.**
///
/// Transcribed from `Processing.NDI.structs.h`. Note `p_data` is `float*` and
/// the channels are PLANAR, one after another with `channel_stride_in_bytes`
/// between them. Everything upstream of here is interleaved 32-bit integer, so
/// something has to convert; see [`Ndi::send_audio_interleaved_32s`] for why
/// that something is the SDK and not us.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct NDIlib_audio_frame_v2_t {
    pub sample_rate: c_int,
    pub no_channels: c_int,
    pub no_samples: c_int,
    pub timecode: i64,
    pub p_data: *mut f32,
    pub channel_stride_in_bytes: c_int,
    pub p_metadata: *const c_char,
    pub timestamp: i64,
}

impl Default for NDIlib_audio_frame_v2_t {
    fn default() -> Self {
        Self {
            sample_rate: 48_000,
            no_channels: 2,
            no_samples: 0,
            // NDIlib_send_timecode_synthesize — let the SDK stamp it, exactly
            // as the video path does, so the two agree by construction.
            timecode: i64::MAX,
            p_data: std::ptr::null_mut(),
            channel_stride_in_bytes: 0,
            p_metadata: std::ptr::null(),
            timestamp: 0,
        }
    }
}

/// Interleaved 32-bit integer audio, the shape everything upstream produces.
///
/// From `Processing.NDI.utilities.h`. `reference_level` is the headroom in dB
/// between the integer full scale and NDI's 0 dB reference — **not a gain
/// control**, and getting it wrong is a silently hot or quiet feed rather than
/// an error.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct NDIlib_audio_frame_interleaved_32s_t {
    pub sample_rate: c_int,
    pub no_channels: c_int,
    pub no_samples: c_int,
    pub timecode: i64,
    pub reference_level: c_int,
    pub p_data: *mut i32,
}

impl Default for NDIlib_audio_frame_interleaved_32s_t {
    fn default() -> Self {
        Self {
            sample_rate: 48_000,
            no_channels: 2,
            no_samples: 0,
            timecode: i64::MAX,
            // 0 dB: the integer full scale IS NDI's reference. The SDK's own
            // examples use 20 for material mastered with 20 dB of headroom;
            // a clip's PCM is already at the level it is meant to be, and
            // adding headroom here would send everything 20 dB quiet.
            reference_level: 0,
            p_data: std::ptr::null_mut(),
        }
    }
}

/// The RECEIVE-side audio frame, and **not the same shape as v2.**
///
/// `NDIlib_recv_capture_v3` takes this, not [`NDIlib_audio_frame_v2_t`]. v3
/// inserts a `FourCC` between `timecode` and `p_data`, so passing a v2 here
/// misreads every field after the timecode — a pointer read as an integer and
/// a stride read as a pointer. It would not crash immediately; it would report
/// a plausible-looking rate and then hand back garbage.
///
/// The two coexist because the SEND path genuinely wants v2:
/// `send_send_audio_v2` and `util_audio_from_interleaved_32s_v2` are both
/// defined in terms of it. Same word, two layouts, one letter apart.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct NDIlib_audio_frame_v3_t {
    pub sample_rate: c_int,
    pub no_channels: c_int,
    pub no_samples: c_int,
    pub timecode: i64,
    pub FourCC: c_int,
    pub p_data: *mut u8,
    /// Union with `data_size_in_bytes` for compressed audio FourCCs.
    pub channel_stride_in_bytes: c_int,
    pub p_metadata: *const c_char,
    pub timestamp: i64,
}

impl Default for NDIlib_audio_frame_v3_t {
    fn default() -> Self {
        Self {
            sample_rate: 48_000,
            no_channels: 2,
            no_samples: 0,
            timecode: i64::MAX,
            FourCC: FOURCC_AUDIO_FLTP,
            p_data: std::ptr::null_mut(),
            channel_stride_in_bytes: 0,
            p_metadata: std::ptr::null(),
            timestamp: 0,
        }
    }
}

pub type find_instance_t = *mut c_void;
pub type recv_instance_t = *mut c_void;
pub type send_instance_t = *mut c_void;

// ---------------------------------------------------------------------------
// The loaded runtime.
// ---------------------------------------------------------------------------

/// A loaded NDI runtime and the symbols we use.
///
/// Every symbol is resolved eagerly at load, so a runtime missing something we
/// need fails immediately and by name, rather than at the moment a service
/// starts.
pub struct Ndi {
    _lib: libloading::Library,
    pub initialize: unsafe extern "C" fn() -> bool,
    pub destroy: unsafe extern "C" fn(),
    pub is_supported_cpu: unsafe extern "C" fn() -> bool,
    pub version: unsafe extern "C" fn() -> *const c_char,

    pub find_create_v2: unsafe extern "C" fn(*const NDIlib_find_create_t) -> find_instance_t,
    pub find_destroy: unsafe extern "C" fn(find_instance_t),
    pub find_wait_for_sources: unsafe extern "C" fn(find_instance_t, u32) -> bool,
    pub find_get_current_sources:
        unsafe extern "C" fn(find_instance_t, *mut u32) -> *const NDIlib_source_t,

    pub recv_create_v3: unsafe extern "C" fn(*const NDIlib_recv_create_v3_t) -> recv_instance_t,
    pub recv_destroy: unsafe extern "C" fn(recv_instance_t),
    pub recv_capture_v3: unsafe extern "C" fn(
        recv_instance_t,
        *mut NDIlib_video_frame_v2_t,
        *mut NDIlib_audio_frame_v3_t,
        *mut c_void,
        u32,
    ) -> c_int,
    pub recv_free_video_v2: unsafe extern "C" fn(recv_instance_t, *const NDIlib_video_frame_v2_t),
    pub recv_free_audio_v3: unsafe extern "C" fn(recv_instance_t, *const NDIlib_audio_frame_v3_t),

    pub send_create: unsafe extern "C" fn(*const NDIlib_send_create_t) -> send_instance_t,
    pub send_destroy: unsafe extern "C" fn(send_instance_t),
    pub send_send_video_v2: unsafe extern "C" fn(send_instance_t, *const NDIlib_video_frame_v2_t),
    pub send_send_audio_v2: unsafe extern "C" fn(send_instance_t, *const NDIlib_audio_frame_v2_t),
    /// Interleaved 32-bit int → planar float, done by the SDK.
    ///
    /// `NDIlib_util_send_send_audio_interleaved_32s` does NOT exist in the
    /// runtime — checked with `nm` against 6.3.2.0, which exports only the
    /// `from_interleaved` converters and `send_send_audio_v2`. So the path is
    /// convert, then send.
    pub util_audio_from_interleaved_32s_v2: unsafe extern "C" fn(
        *const NDIlib_audio_frame_interleaved_32s_t,
        *mut NDIlib_audio_frame_v2_t,
    ),
    pub send_get_no_connections: unsafe extern "C" fn(send_instance_t, u32) -> c_int,
}

#[derive(Debug, thiserror::Error)]
pub enum NdiError {
    #[error("NDI runtime not found. Tried: {0}")]
    NotFound(String),
    #[error("NDI runtime at {path} is missing symbol {symbol}")]
    MissingSymbol { path: String, symbol: String },
    #[error(
        "NDIlib_initialize() returned false — this CPU is unsupported, or the license check failed"
    )]
    InitFailed,
}

macro_rules! sym {
    ($lib:expr, $path:expr, $name:literal) => {{
        let name: &[u8] = concat!($name, "\0").as_bytes();
        // SAFETY: the symbol's type is transcribed from include/pmg_ndi.h,
        // which was verified against the SDK headers.
        let s: libloading::Symbol<_> =
            unsafe { $lib.get(name) }.map_err(|_| NdiError::MissingSymbol {
                path: $path.to_string(),
                symbol: $name.to_string(),
            })?;
        *s
    }};
}

impl Ndi {
    /// Load from an explicit path.
    pub fn load_from(path: &str) -> Result<Self, NdiError> {
        // SAFETY: loading a shared library runs its initialisers. This is the
        // vendor runtime, at a path the operator configured.
        let lib = unsafe { libloading::Library::new(path) }
            .map_err(|_| NdiError::NotFound(path.to_string()))?;

        let ndi = Self {
            initialize: sym!(lib, path, "NDIlib_initialize"),
            destroy: sym!(lib, path, "NDIlib_destroy"),
            is_supported_cpu: sym!(lib, path, "NDIlib_is_supported_CPU"),
            version: sym!(lib, path, "NDIlib_version"),
            find_create_v2: sym!(lib, path, "NDIlib_find_create_v2"),
            find_destroy: sym!(lib, path, "NDIlib_find_destroy"),
            find_wait_for_sources: sym!(lib, path, "NDIlib_find_wait_for_sources"),
            find_get_current_sources: sym!(lib, path, "NDIlib_find_get_current_sources"),
            recv_create_v3: sym!(lib, path, "NDIlib_recv_create_v3"),
            recv_destroy: sym!(lib, path, "NDIlib_recv_destroy"),
            recv_capture_v3: sym!(lib, path, "NDIlib_recv_capture_v3"),
            recv_free_video_v2: sym!(lib, path, "NDIlib_recv_free_video_v2"),
            recv_free_audio_v3: sym!(lib, path, "NDIlib_recv_free_audio_v3"),
            send_create: sym!(lib, path, "NDIlib_send_create"),
            send_destroy: sym!(lib, path, "NDIlib_send_destroy"),
            send_send_video_v2: sym!(lib, path, "NDIlib_send_send_video_v2"),
            send_send_audio_v2: sym!(lib, path, "NDIlib_send_send_audio_v2"),
            util_audio_from_interleaved_32s_v2: sym!(
                lib,
                path,
                "NDIlib_util_audio_from_interleaved_32s_v2"
            ),
            send_get_no_connections: sym!(lib, path, "NDIlib_send_get_no_connections"),
            _lib: lib,
        };

        // SAFETY: symbols resolved above; no arguments.
        unsafe {
            if !(ndi.initialize)() {
                return Err(NdiError::InitFailed);
            }
        }
        Ok(ndi)
    }

    /// Try each candidate path in order.
    pub fn load() -> Result<Self, NdiError> {
        if let Ok(p) = std::env::var("PMG_NDI_RUNTIME") {
            return Self::load_from(&p);
        }
        for path in RUNTIME_CANDIDATES {
            if let Ok(ndi) = Self::load_from(path) {
                return Ok(ndi);
            }
        }
        Err(NdiError::NotFound(RUNTIME_CANDIDATES.join(", ")))
    }

    /// Runtime version string, for `docs/INTEGRATION-NOTES.md`.
    pub fn version_string(&self) -> String {
        // SAFETY: the runtime owns this string and it is valid for its lifetime.
        unsafe {
            let p = (self.version)();
            if p.is_null() {
                "unknown".into()
            } else {
                CStr::from_ptr(p).to_string_lossy().into_owned()
            }
        }
    }

    pub fn supported_cpu(&self) -> bool {
        // SAFETY: no arguments, no state.
        unsafe { (self.is_supported_cpu)() }
    }
}

impl Drop for Ndi {
    fn drop(&mut self) {
        // SAFETY: paired with the initialize() in load_from().
        unsafe { (self.destroy)() }
    }
}

/// Whether a runtime can be found without loading it.
pub fn runtime_available() -> bool {
    std::env::var("PMG_NDI_RUNTIME")
        .ok()
        .into_iter()
        .chain(RUNTIME_CANDIDATES.iter().map(|s| s.to_string()))
        .any(|p| std::path::Path::new(&p).exists())
}

// ---------------------------------------------------------------------------
// ABI conformance.
//
// These numbers came from a C probe compiled against the real SDK headers
// (arm64, macOS 26.3, 2026-08-14) and recorded in include/pmg_ndi.h. Asserting
// against recorded constants rather than against headers means this test runs
// everywhere — including CI runners with no NDI SDK installed, which is the
// whole point.
//
// If one of these ever fails, the struct above is wrong and sending a frame
// would be a crash or garbage, not a compile error. That is why they exist.
// ---------------------------------------------------------------------------
#[cfg(test)]
mod abi {
    use super::*;
    use std::mem::{align_of, offset_of, size_of};

    #[test]
    fn video_frame_v2_layout() {
        assert_eq!(size_of::<NDIlib_video_frame_v2_t>(), 72);
        assert_eq!(align_of::<NDIlib_video_frame_v2_t>(), 8);
        assert_eq!(offset_of!(NDIlib_video_frame_v2_t, xres), 0);
        assert_eq!(offset_of!(NDIlib_video_frame_v2_t, yres), 4);
        assert_eq!(offset_of!(NDIlib_video_frame_v2_t, FourCC), 8);
        assert_eq!(offset_of!(NDIlib_video_frame_v2_t, frame_rate_N), 12);
        assert_eq!(offset_of!(NDIlib_video_frame_v2_t, frame_rate_D), 16);
        assert_eq!(
            offset_of!(NDIlib_video_frame_v2_t, picture_aspect_ratio),
            20
        );
        assert_eq!(offset_of!(NDIlib_video_frame_v2_t, frame_format_type), 24);
        assert_eq!(offset_of!(NDIlib_video_frame_v2_t, timecode), 32);
        assert_eq!(offset_of!(NDIlib_video_frame_v2_t, p_data), 40);
        assert_eq!(
            offset_of!(NDIlib_video_frame_v2_t, line_stride_in_bytes),
            48
        );
        assert_eq!(offset_of!(NDIlib_video_frame_v2_t, p_metadata), 56);
        assert_eq!(offset_of!(NDIlib_video_frame_v2_t, timestamp), 64);

        // Audio. Same discipline: the C compiler decides the padding and these
        // confirm Rust put it in the same places. A wrong offset here is not a
        // crash, it is audio at the wrong rate or channel count — which sounds
        // like a broken decoder and is not one.
        assert_eq!(offset_of!(NDIlib_audio_frame_v2_t, sample_rate), 0);
        assert_eq!(offset_of!(NDIlib_audio_frame_v2_t, no_channels), 4);
        assert_eq!(offset_of!(NDIlib_audio_frame_v2_t, no_samples), 8);
        assert_eq!(offset_of!(NDIlib_audio_frame_v2_t, timecode), 16);
        assert_eq!(offset_of!(NDIlib_audio_frame_v2_t, p_data), 24);
        assert_eq!(
            offset_of!(NDIlib_audio_frame_v2_t, channel_stride_in_bytes),
            32
        );
        assert_eq!(offset_of!(NDIlib_audio_frame_v2_t, p_metadata), 40);
        assert_eq!(offset_of!(NDIlib_audio_frame_v2_t, timestamp), 48);
        assert_eq!(std::mem::size_of::<NDIlib_audio_frame_v2_t>(), 56);

        // v3, the RECEIVE shape. The FourCC at 24 is the whole difference, and
        // it pushes everything after it along by eight bytes.
        assert_eq!(offset_of!(NDIlib_audio_frame_v3_t, sample_rate), 0);
        assert_eq!(offset_of!(NDIlib_audio_frame_v3_t, no_channels), 4);
        assert_eq!(offset_of!(NDIlib_audio_frame_v3_t, no_samples), 8);
        assert_eq!(offset_of!(NDIlib_audio_frame_v3_t, timecode), 16);
        assert_eq!(offset_of!(NDIlib_audio_frame_v3_t, FourCC), 24);
        assert_eq!(offset_of!(NDIlib_audio_frame_v3_t, p_data), 32);
        assert_eq!(
            offset_of!(NDIlib_audio_frame_v3_t, channel_stride_in_bytes),
            40
        );
        assert_eq!(offset_of!(NDIlib_audio_frame_v3_t, p_metadata), 48);
        assert_eq!(offset_of!(NDIlib_audio_frame_v3_t, timestamp), 56);
        assert_eq!(std::mem::size_of::<NDIlib_audio_frame_v3_t>(), 64);
        // The v2 send shape and the v3 receive shape MUST differ. If a refactor
        // ever makes them agree, one of them is wrong.
        assert_ne!(
            std::mem::size_of::<NDIlib_audio_frame_v2_t>(),
            std::mem::size_of::<NDIlib_audio_frame_v3_t>()
        );

        assert_eq!(
            offset_of!(NDIlib_audio_frame_interleaved_32s_t, sample_rate),
            0
        );
        assert_eq!(
            offset_of!(NDIlib_audio_frame_interleaved_32s_t, no_channels),
            4
        );
        assert_eq!(
            offset_of!(NDIlib_audio_frame_interleaved_32s_t, no_samples),
            8
        );
        assert_eq!(
            offset_of!(NDIlib_audio_frame_interleaved_32s_t, timecode),
            16
        );
        assert_eq!(
            offset_of!(NDIlib_audio_frame_interleaved_32s_t, reference_level),
            24
        );
        assert_eq!(offset_of!(NDIlib_audio_frame_interleaved_32s_t, p_data), 32);
        assert_eq!(
            std::mem::size_of::<NDIlib_audio_frame_interleaved_32s_t>(),
            40
        );
    }

    #[test]
    fn send_create_layout() {
        assert_eq!(size_of::<NDIlib_send_create_t>(), 24);
        assert_eq!(offset_of!(NDIlib_send_create_t, p_ndi_name), 0);
        assert_eq!(offset_of!(NDIlib_send_create_t, p_groups), 8);
        assert_eq!(offset_of!(NDIlib_send_create_t, clock_video), 16);
        assert_eq!(offset_of!(NDIlib_send_create_t, clock_audio), 17);
    }

    #[test]
    fn source_and_find_layout() {
        assert_eq!(size_of::<NDIlib_source_t>(), 16);
        assert_eq!(offset_of!(NDIlib_source_t, p_url_address), 8);
        assert_eq!(size_of::<NDIlib_find_create_t>(), 24);
        assert_eq!(offset_of!(NDIlib_find_create_t, show_local_sources), 0);
        assert_eq!(offset_of!(NDIlib_find_create_t, p_groups), 8);
        assert_eq!(offset_of!(NDIlib_find_create_t, p_extra_ips), 16);
    }

    #[test]
    fn recv_create_v3_layout() {
        assert_eq!(size_of::<NDIlib_recv_create_v3_t>(), 40);
        assert_eq!(offset_of!(NDIlib_recv_create_v3_t, source_to_connect_to), 0);
        assert_eq!(offset_of!(NDIlib_recv_create_v3_t, color_format), 16);
        assert_eq!(offset_of!(NDIlib_recv_create_v3_t, bandwidth), 20);
        assert_eq!(offset_of!(NDIlib_recv_create_v3_t, allow_video_fields), 24);
        assert_eq!(offset_of!(NDIlib_recv_create_v3_t, p_ndi_recv_name), 32);
    }

    #[test]
    fn constants_match_the_sdk() {
        // NDI_LIB_FOURCC('B','G','R','A') = 'B' | 'G'<<8 | 'R'<<16 | 'A'<<24
        let expect = (b'B' as c_int)
            | ((b'G' as c_int) << 8)
            | ((b'R' as c_int) << 16)
            | ((b'A' as c_int) << 24);
        assert_eq!(FOURCC_BGRA, expect);
        assert_eq!(FOURCC_BGRA, 0x4152_4742);
        assert_eq!(FRAME_FORMAT_PROGRESSIVE, 1);
        assert_eq!(FRAME_TYPE_VIDEO, 1);
        assert_eq!(FRAME_TYPE_AUDIO, 2);
        assert_eq!(RECV_COLOR_FORMAT_BGRX_BGRA, 0);
        assert_eq!(RECV_BANDWIDTH_HIGHEST, 100);
    }

    #[test]
    fn header_is_the_contract_of_record() {
        let h = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("include/pmg_ndi.h");
        assert!(
            h.exists(),
            "include/pmg_ndi.h is the contract of record and must exist"
        );
    }
}
