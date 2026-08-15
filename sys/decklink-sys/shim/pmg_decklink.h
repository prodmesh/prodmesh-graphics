/*
 * pmg_decklink.h — hand-written C header. THE CONTRACT OF RECORD.
 *
 * This file is written by hand and is authoritative. It is not generated, and
 * bindgen is never pointed at the DeckLink SDK's C++ headers — those are COM on
 * Windows and a hand-rolled COM-alike elsewhere, with IUnknown refcounting,
 * QueryInterface, and virtual interfaces you are expected to *implement*.
 * Generating bindings for that is misery, and the generated result would not be
 * a contract anyone could read.
 *
 * This is the same pattern as prodmesh-caption's swift/include/pmcaption_speech.h.
 * See ADR 0003 and risk R4.
 *
 * The Rust side (src/lib.rs) declares exactly these symbols by hand. If the two
 * disagree, this file is right.
 *
 * ---------------------------------------------------------------------------
 * RULES THE IMPLEMENTATION MUST HOLD
 *
 * 1. The frame-completion callback arrives on DeckLink's own thread.
 *    It must NEVER allocate, NEVER block, and NEVER take a lock. It pushes a
 *    token into a bounded queue and returns. One violation is a late frame,
 *    and a late frame is a visible glitch.
 *
 * 2. Every interface pointer obtained from the SDK is released exactly once.
 *    On the Rust side each is wrapped in a type whose Drop calls the matching
 *    pmg_dl_*_release. This shim owns the refcounting; Rust owns the lifetime.
 *
 * 3. Capability is PROBED, never assumed. Device capability varies wildly
 *    within a product line, and the model number does not predict whether
 *    key+fill is available or which ports pair.
 *
 * 4. For key+fill, both ports are scheduled at the SAME display time on the
 *    SAME device. Same device means same clock, which is what actually
 *    guarantees phase (ADR 0011).
 *
 * 5. No function here allocates memory that Rust must free, and none takes
 *    ownership of a Rust allocation. Strings out are into caller-provided
 *    buffers with an explicit capacity.
 * ---------------------------------------------------------------------------
 *
 * STATUS: declared, not implemented. Spike S3 (docs/spikes/S3-decklink-output.md)
 * builds the implementation and answers whether this approach holds at all.
 */

#ifndef PMG_DECKLINK_H
#define PMG_DECKLINK_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* ---- Result codes. Zero is success; negative is failure. ---------------- */
typedef int32_t pmg_dl_result;
#define PMG_DL_OK                 0
#define PMG_DL_ERR_NO_SDK        -1  /* SDK absent at build or load time     */
#define PMG_DL_ERR_NO_DEVICE     -2
#define PMG_DL_ERR_BAD_MODE      -3
#define PMG_DL_ERR_NO_KEY_FILL   -4  /* device cannot do key+fill on a pair  */
#define PMG_DL_ERR_BUSY          -5
#define PMG_DL_ERR_INTERNAL      -6

/* ---- Frame completion, as reported by the SDK. -------------------------- */
typedef enum {
    PMG_DL_FRAME_COMPLETED      = 0,
    PMG_DL_FRAME_DISPLAYED_LATE = 1,  /* the one to count in spike S3        */
    PMG_DL_FRAME_DROPPED        = 2,
    PMG_DL_FRAME_FLUSHED        = 3
} pmg_dl_completion;

/* ---- Opaque handles. Rust never dereferences these. --------------------- */
typedef struct pmg_dl_device pmg_dl_device;
typedef struct pmg_dl_output pmg_dl_output;

/* ---- Device enumeration and capability probing -------------------------- */

typedef struct {
    char     display_name[128];
    char     model_name[128];
    int32_t  index;
    int32_t  sub_device_count;    /* connectors on this physical card        */
    int32_t  sub_device_index;    /* which connector this entry is, or -1    */
    int32_t  supports_key_fill;   /* external keying, 0 or 1, PROBED         */
    int32_t  supports_internal_key; /* 0 or 1, PROBED                        */
    int32_t  has_reference_input; /* 0 or 1                                  */
    int32_t  max_audio_channels;
    int64_t  persistent_id;       /* stable across reboots; PER CONNECTOR    */
    int64_t  topological_id;      /* identifies the physical CARD            */
} pmg_dl_device_info;

/*
 * On pairing: SDK 16.0 has no "paired device" attribute, so grouping is ours to
 * do. Group on topological_id, NOT persistent_id.
 *
 * Measured on a DeckLink Quad 2, 2026-08-14: persistent_id increments PER
 * CONNECTOR (0xC55080A0 .. 0xC55080A7), so grouping on it invents eight
 * single-connector "cards". topological_id is the physical card, and
 * sub_device_index orders the connectors within it.
 *
 * This matters because a fill+key pair must be two connectors on ONE card —
 * same card, same clock, which is what actually guarantees phase (ADR 0011).
 * Grouping on the wrong field would let Configure offer a "pair" spanning two
 * cards, which would look fine and be out of phase.
 */

/* Number of devices present. Safe to call before anything else. */
int32_t pmg_dl_device_count(void);

/* Fill `out` for device `index`. */
pmg_dl_result pmg_dl_device_info_at(int32_t index, pmg_dl_device_info *out);

pmg_dl_result pmg_dl_device_open(int32_t index, pmg_dl_device **out);
void          pmg_dl_device_release(pmg_dl_device *dev);

/* ---- Display modes ------------------------------------------------------ */

typedef struct {
    int32_t width;
    int32_t height;
    int32_t fps_num;      /* exact rational, e.g. 60000                      */
    int32_t fps_den;      /* e.g. 1001. NEVER a float frame rate.            */
    int32_t interlaced;   /* 0 or 1                                          */
    char    name[64];     /* the SDK's own name, for display in Configure    */
} pmg_dl_mode;

int32_t       pmg_dl_mode_count(pmg_dl_device *dev);
pmg_dl_result pmg_dl_mode_at(pmg_dl_device *dev, int32_t i, pmg_dl_mode *out);

/* ---- Reference (genlock) ------------------------------------------------
 *
 * NOTE: reference gives PHASE, not frame numbering. Two nodes locked to the
 * same generator emit frames in lockstep but do not agree on which frame is
 * number 1841944. See ADR 0011 — this comment exists because someone will
 * assume otherwise.
 */
typedef struct {
    int32_t present;      /* device has a reference input                    */
    int32_t locked;
    char    format[64];   /* e.g. "1080i5994", empty if unlocked             */
} pmg_dl_reference;

pmg_dl_result pmg_dl_reference_status(pmg_dl_device *dev, pmg_dl_reference *out);

/* ---- Output ------------------------------------------------------------- */

typedef enum {
    PMG_DL_KEY_FILL_ONLY     = 0,
    PMG_DL_KEY_KEY_ONLY      = 1,
    PMG_DL_KEY_FILL_KEY_PAIR = 2,  /* two ports, one device, same clock      */
    PMG_DL_KEY_EMBEDDED      = 3
} pmg_dl_key_mode;

typedef struct {
    pmg_dl_mode     mode;
    pmg_dl_key_mode key_mode;
    int32_t         paired_port;     /* FILL_KEY_PAIR only                   */
    int32_t         use_reference;
    int32_t         preroll_frames;  /* 3-5 typical; spike S3 measures ours  */
    int32_t         audio_channels;  /* 0 for none                           */
} pmg_dl_output_config;

pmg_dl_result pmg_dl_output_open(pmg_dl_device *dev,
                                 const pmg_dl_output_config *cfg,
                                 pmg_dl_output **out);
void          pmg_dl_output_release(pmg_dl_output *out);

/*
 * Frame completion callback.
 *
 * CALLED ON DECKLINK'S THREAD. Allocating, blocking, or locking here is a
 * late frame. The implementation pushes `token` and `result` into a bounded
 * queue and returns immediately. `user` is an opaque Rust pointer.
 */
typedef void (*pmg_dl_frame_completed_cb)(void *user,
                                          uint64_t token,
                                          pmg_dl_completion result);

pmg_dl_result pmg_dl_output_set_callback(pmg_dl_output *out,
                                         pmg_dl_frame_completed_cb cb,
                                         void *user);

/*
 * Schedule one frame.
 *
 * `fill` is premultiplied-over-black BGRA/v210 per the configured mode.
 * `key` is the alpha plane and is NULL unless key_mode is FILL_KEY_PAIR.
 *
 * The key is NOT a colour: it does not go through a colour matrix. The caller
 * (pmg-color) has already written Y = alpha with neutral chroma. See ADR 0012.
 *
 * `display_time` is in `time_scale` units. For a fill+key pair, BOTH ports are
 * scheduled at the same display_time — that is what guarantees phase.
 *
 * `token` is echoed back to the completion callback so the caller can retire
 * its buffer.
 */
pmg_dl_result pmg_dl_output_schedule(pmg_dl_output *out,
                                     const void *fill, int32_t fill_stride,
                                     const void *key,  int32_t key_stride,
                                     const void *audio, int32_t audio_samples,
                                     int64_t display_time, int64_t time_scale,
                                     uint64_t token);

pmg_dl_result pmg_dl_output_start(pmg_dl_output *out);
pmg_dl_result pmg_dl_output_stop(pmg_dl_output *out);

/* Counters since start. Spike S3's success bar is zero late and zero dropped
 * over ten minutes at 1080p59.94. */
typedef struct {
    uint64_t scheduled;
    uint64_t completed;
    uint64_t displayed_late;
    uint64_t dropped;
    uint64_t buffered;
} pmg_dl_stats;

pmg_dl_result pmg_dl_output_stats(pmg_dl_output *out, pmg_dl_stats *stats);

/* ---- Build / runtime introspection -------------------------------------- */

/* 1 if this shim was compiled against a real SDK, 0 if it is the stub build.
 * `cargo build --workspace` must succeed on a machine that has never seen the
 * DeckLink SDK, so the stub build is a supported configuration. */
int32_t pmg_dl_available(void);

/* SDK version string, or "stub". Written into caller's buffer. */
void pmg_dl_sdk_version(char *buf, int32_t cap);

/*
 * Which IID generation the DRIVER actually satisfied, per interface, after the
 * newest-first fallback in the shim. E.g. "attrs=15.3.1 status=15.3.1 output=14.2.1".
 *
 * This exists because the failure it diagnoses is invisible: when the driver is
 * older than the SDK, QueryInterface returns E_NOINTERFACE and every capability
 * silently reads as its default, which looks exactly like a card that cannot do
 * anything. Call it before believing a "no" from pmg_dl_device_info_at.
 */
void pmg_dl_interface_report(char *buf, int32_t cap);

#ifdef __cplusplus
}
#endif
#endif /* PMG_DECKLINK_H */
