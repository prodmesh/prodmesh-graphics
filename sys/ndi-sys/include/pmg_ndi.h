/*
 * pmg_ndi.h — hand-written C header. THE CONTRACT OF RECORD.
 *
 * The NDI SDK headers are not installed on this machine and are not vendored.
 * This file transcribes, by hand, exactly the subset of the NDI API this
 * project uses. The Rust declarations in src/lib.rs are transcribed from THIS
 * FILE, and if the two ever disagree, this file is right.
 *
 * Same pattern as sys/decklink-sys/shim/pmg_decklink.h and prodmesh-caption's
 * swift/include/pmcaption_speech.h. See ADR 0003.
 *
 * ---------------------------------------------------------------------------
 * WHY THIS IS SAFER THAN IT LOOKS, AND WHERE IT IS STILL RISKY
 *
 * The runtime exports FLAT C SYMBOLS (verified with nm: NDIlib_send_create,
 * NDIlib_send_send_video_v2, NDIlib_recv_capture_v3, ...), not only the
 * NDIlib_v5_load() function-pointer table. That matters: a mistake in a
 * function signature surfaces as a missing symbol, not as a silently wrong
 * offset into a 100-entry vtable.
 *
 * The remaining risk is STRUCT LAYOUT. These structs are passed by pointer and
 * a wrong field order or type is a crash or garbage, not a compile error.
 *
 * So the layouts here are not eyeballed — they are VERIFIED. A C probe that
 * includes the real SDK headers emitted every offset, and those numbers are
 * asserted against Rust's offset_of! in src/lib.rs. That test runs on every
 * build, on machines with no SDK installed, because it compares against
 * recorded constants rather than against headers.
 *
 * Verified 2026-08-14 against NDI SDK for Apple, arm64, macOS 26.3:
 *   NDIlib_video_frame_v2_t  size 72  align 8
 *     xres 0, yres 4, FourCC 8, frame_rate_N 12, frame_rate_D 16,
 *     picture_aspect_ratio 20, frame_format_type 24, [pad], timecode 32,
 *     p_data 40, line_stride_in_bytes 48, [pad], p_metadata 56, timestamp 64
 *   NDIlib_send_create_t     size 24  (16, 17 are the two bools)
 *   NDIlib_source_t          size 16
 *   NDIlib_find_create_t     size 24  (bool at 0, then 8, 16)
 *   NDIlib_recv_create_v3_t  size 40  (source 0..16, 16, 20, 24, 32)
 * ---------------------------------------------------------------------------
 *
 * The runtime is LOADED AT RUNTIME, never linked, so redistribution stays a
 * deployment question rather than a build-time commitment (ADR 0003 §Seam).
 */

#ifndef PMG_NDI_H
#define PMG_NDI_H

#include <stdbool.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* NDI_LIB_FOURCC(a,b,c,d) = a | b<<8 | c<<16 | d<<24 */
#define PMG_NDI_FOURCC_BGRA 0x41524742 /* 'B','G','R','A' */
#define PMG_NDI_FOURCC_RGBA 0x41424752 /* 'R','G','B','A' */
#define PMG_NDI_FOURCC_UYVY 0x59565955 /* 'U','Y','V','Y' */

typedef enum {
    NDIlib_frame_type_none          = 0,
    NDIlib_frame_type_video         = 1,
    NDIlib_frame_type_audio         = 2,
    NDIlib_frame_type_metadata      = 3,
    NDIlib_frame_type_error         = 4,
    NDIlib_frame_type_status_change = 100
} NDIlib_frame_type_e;

typedef enum {
    NDIlib_frame_format_type_progressive  = 1,
    NDIlib_frame_format_type_interleaved  = 0,
    NDIlib_frame_format_type_field_0      = 2,
    NDIlib_frame_format_type_field_1      = 3
} NDIlib_frame_format_type_e;

typedef enum {
    NDIlib_recv_color_format_BGRX_BGRA = 0,
    NDIlib_recv_color_format_UYVY_BGRA = 1,
    NDIlib_recv_color_format_RGBX_RGBA = 2,
    NDIlib_recv_color_format_UYVY_RGBA = 3,
    NDIlib_recv_color_format_fastest   = 100,
    NDIlib_recv_color_format_best      = 101
} NDIlib_recv_color_format_e;

typedef enum {
    NDIlib_recv_bandwidth_metadata_only = -10,
    NDIlib_recv_bandwidth_audio_only    = 10,
    NDIlib_recv_bandwidth_lowest        = 0,
    NDIlib_recv_bandwidth_highest       = 100
} NDIlib_recv_bandwidth_e;

/* ---- Structures. Field ORDER and TYPE are load-bearing. ----------------- */

typedef struct NDIlib_source_t {
    const char *p_ndi_name;
    const char *p_url_address; /* union with p_ip_address; same offset */
} NDIlib_source_t;

typedef struct NDIlib_find_create_t {
    bool        show_local_sources;
    const char *p_groups;
    const char *p_extra_ips;
} NDIlib_find_create_t;

typedef struct NDIlib_recv_create_v3_t {
    NDIlib_source_t            source_to_connect_to;
    NDIlib_recv_color_format_e color_format;
    NDIlib_recv_bandwidth_e    bandwidth;
    bool                       allow_video_fields;
    const char                *p_ndi_recv_name;
} NDIlib_recv_create_v3_t;

typedef struct NDIlib_send_create_t {
    const char *p_ndi_name;
    const char *p_groups;
    bool        clock_video; /* true = the SDK paces us to frame_rate         */
    bool        clock_audio;
} NDIlib_send_create_t;

/*
 * The struct this whole exercise is about.
 *
 * On arm64 the compiler inserts 4 bytes of padding after frame_format_type to
 * align timecode, and 4 more after line_stride_in_bytes to align p_metadata.
 * Rust's #[repr(C)] does the same, which is why the Rust side declares the
 * fields in this order and lets the compiler place them rather than hard-coding
 * offsets.
 */
typedef struct NDIlib_video_frame_v2_t {
    int                        xres;
    int                        yres;
    /* NDIlib_FourCC_video_type_e in the SDK; an int-sized enum, so ABI-identical. */
    int                        FourCC;
    int                        frame_rate_N;
    int                        frame_rate_D;
    float                      picture_aspect_ratio;
    NDIlib_frame_format_type_e frame_format_type;
    int64_t                    timecode;
    uint8_t                   *p_data;
    int                        line_stride_in_bytes; /* union w/ data_size_in_bytes */
    const char                *p_metadata;
    int64_t                    timestamp;
} NDIlib_video_frame_v2_t;

typedef struct NDIlib_audio_frame_v3_t  NDIlib_audio_frame_v3_t;
typedef struct NDIlib_metadata_frame_t  NDIlib_metadata_frame_t;

/* ---- Lifecycle ---------------------------------------------------------- */

bool NDIlib_initialize(void);
void NDIlib_destroy(void);
bool NDIlib_is_supported_CPU(void);
const char *NDIlib_version(void);

/* ---- Find (used by `pmgctl ndi probe` to locate a validation source) ----- */

typedef struct NDIlib_find_instance_type *NDIlib_find_instance_t;

NDIlib_find_instance_t NDIlib_find_create_v2(const NDIlib_find_create_t *p_create);
void                   NDIlib_find_destroy(NDIlib_find_instance_t p_instance);
bool                   NDIlib_find_wait_for_sources(NDIlib_find_instance_t p_instance,
                                                    uint32_t timeout_ms);
/* Returned array is owned by the finder and is valid until the next call. */
const NDIlib_source_t *NDIlib_find_get_current_sources(NDIlib_find_instance_t p_instance,
                                                       uint32_t *p_no_sources);

/* ---- Receive — THE ABI VALIDATION PATH ---------------------------------- */

typedef struct NDIlib_recv_instance_type *NDIlib_recv_instance_t;

NDIlib_recv_instance_t NDIlib_recv_create_v3(const NDIlib_recv_create_v3_t *p_create);
void                   NDIlib_recv_destroy(NDIlib_recv_instance_t p_instance);

NDIlib_frame_type_e NDIlib_recv_capture_v3(NDIlib_recv_instance_t p_instance,
                                           NDIlib_video_frame_v2_t *p_video,
                                           NDIlib_audio_frame_v3_t *p_audio,
                                           NDIlib_metadata_frame_t *p_metadata,
                                           uint32_t timeout_ms);

void NDIlib_recv_free_video_v2(NDIlib_recv_instance_t p_instance,
                               const NDIlib_video_frame_v2_t *p_video);

/* ---- Send --------------------------------------------------------------- */

typedef struct NDIlib_send_instance_type *NDIlib_send_instance_t;

NDIlib_send_instance_t NDIlib_send_create(const NDIlib_send_create_t *p_create);
void                   NDIlib_send_destroy(NDIlib_send_instance_t p_instance);

/*
 * Synchronous send. With clock_video = true the SDK blocks to pace us at
 * frame_rate_N/frame_rate_D, which is what makes a sender free-run at the
 * right rate without our own timer.
 *
 * ALPHA: NDI wants STRAIGHT alpha. Everything internal to this project is
 * premultiplied, so pmg_color::unpremultiply runs at this boundary. Getting the
 * direction wrong produces fringing that is invisible over one background and
 * obvious over the other — which is exactly what the `softedge` test pattern
 * exists to catch (docs/specs/color-and-key.md §5).
 */
void NDIlib_send_send_video_v2(NDIlib_send_instance_t p_instance,
                               const NDIlib_video_frame_v2_t *p_video);

int NDIlib_send_get_no_connections(NDIlib_send_instance_t p_instance,
                                   uint32_t timeout_ms);

#ifdef __cplusplus
}
#endif
#endif /* PMG_NDI_H */
