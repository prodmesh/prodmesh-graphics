/*
 * pmg_decklink.cpp — the shim implementing pmg_decklink.h.
 *
 * All of the DeckLink SDK's COM-alike ugliness lives here and nowhere else:
 * IUnknown refcounting, QueryInterface, CFStringRef, and the virtual interfaces
 * you are expected to implement. Rust sees only the flat extern "C" API in
 * pmg_decklink.h, which is the contract of record. See ADR 0003 and risk R4.
 *
 * DeckLinkAPIDispatch.cpp (compiled alongside this file) provides
 * CreateDeckLinkIteratorInstance(), which dlopens DeckLinkAPI.framework at
 * runtime. So this shim links against no Blackmagic library — the framework is
 * a runtime dependency, which keeps the vendor boundary at the seam ADR 0003
 * describes.
 *
 * M0 SCOPE: enumeration, capability probing, display modes, and reference
 * status. That is the half of the header that answers real questions for
 * docs/INTEGRATION-NOTES.md — which device, which modes, whether key+fill is
 * actually available and on which port pair. Scheduled output is spike S3's
 * job and is stubbed below.
 */

#include "pmg_decklink.h"

#include <atomic>
#include <cstring>
#include <string>
#include <vector>

#include "DeckLinkAPI.h"
#include "DeckLinkAPIVersion.h"
/* Versioned interfaces, for the fallback chain below. The Output IIDs live in
 * their own DeckLinkAPIVideoOutput_v*.h files, not in DeckLinkAPI_v*.h. */
#include "DeckLinkAPI_v15_3_1.h"
#include "DeckLinkAPI_v10_11.h"
#include "DeckLinkAPIVideoOutput_v15_3_1.h"
#include "DeckLinkAPIVideoOutput_v14_2_1.h"
#include "DeckLinkAPIVideoFrame_v14_2_1.h"
#include "DeckLinkAPIVideoOutput_v11_4.h"
#include "DeckLinkAPIVideoOutput_v10_11.h"

namespace {

/* macOS hands back CFStringRef; convert and release. Never leak these — an
 * enumeration that runs on a timer would otherwise bleed. */
std::string from_cf(CFStringRef s) {
    if (!s) return {};
    char buf[512] = {0};
    CFStringGetCString(s, buf, sizeof(buf), kCFStringEncodingUTF8);
    CFRelease(s);
    return std::string(buf);
}

void copy_into(char *dst, int cap, const std::string &src) {
    if (!dst || cap <= 0) return;
    std::snprintf(dst, static_cast<size_t>(cap), "%s", src.c_str());
}

/* One enumerated device. We hold the IDeckLink* and release it in the
 * destructor — rule 2 in the header. */
struct Device {
    IDeckLink *dl = nullptr;
    ~Device() {
        if (dl) dl->Release();
    }
};

/* The iterator is re-created per enumeration rather than cached: a device can
 * be hot-plugged over Thunderbolt, and a cached iterator would go stale in a
 * way that is very confusing to debug at 10am on a Saturday. */
std::vector<IDeckLink *> enumerate() {
    std::vector<IDeckLink *> out;
    IDeckLinkIterator *it = CreateDeckLinkIteratorInstance();
    if (!it) return out; /* driver absent */
    IDeckLink *dl = nullptr;
    while (it->Next(&dl) == S_OK) out.push_back(dl);
    it->Release();
    return out;
}

void release_all(std::vector<IDeckLink *> &v) {
    for (auto *d : v) d->Release();
    v.clear();
}

/* ---------------------------------------------------------------------------
 * VERSION SKEW. Read this before touching anything below.
 *
 * The SDK you compile against and the DRIVER installed on the machine are
 * different things and routinely different versions. Blackmagic mints a NEW
 * IID whenever an interface changes, so an SDK-16.0 IID simply does not exist
 * in a Desktop Video 14.5 driver: QueryInterface returns E_NOINTERFACE.
 *
 * The dangerous part is what that looks like. Every attribute read then falls
 * back to its default, and a DeckLink Quad 2 reports no keying, no audio
 * channels, and persistent id 0 — indistinguishable from "this card can't do
 * anything". Measured on this machine 2026-08-14; see docs/INTEGRATION-NOTES.md.
 *
 * So: try IIDs newest-first and fall back. That is safe ONLY because the
 * methods we call sit at IDENTICAL vtable slots in every version in each chain:
 *
 *   Attributes  GetFlag=0, GetInt=1          identical in current and v15_3_1
 *   Status      GetFlag=0, GetInt=1          identical in current and v15_3_1
 *   Output      GetDisplayModeIterator=2     identical in current, v15_3_1,
 *                                            v14_2_1, v11_5_1, v10_11
 *
 * >>> IF YOU CALL A METHOD PAST THE COMMON PREFIX, THIS BECOMES A SILENT
 * >>> VTABLE MISMATCH AND WILL CRASH OR CORRUPT. IDeckLinkOutput diverges at
 * >>> slot 6 (v14_2_1 inserts SetVideoOutputFrameMemoryAllocator). Before
 * >>> adding a call, verify its slot in EVERY version in the chain, or narrow
 * >>> the chain.
 *
 * Spike S3's output path should narrow to one version and require a matching
 * driver rather than extend this. Enumeration is a different case: it must work
 * on whatever is in the building.
 * ------------------------------------------------------------------------- */

/* Which IID satisfied each interface, for diagnostics. */
/* "unprobed" until the interface is actually queried; "none" only means a
 * query was made and every IID in the chain was refused. Conflating the two
 * would cry wolf about a capability nobody has asked for yet. */
char g_iface_attrs[64]  = "unprobed";
char g_iface_status[64] = "unprobed";
char g_iface_output[64] = "unprobed";

struct IidCandidate {
    REFIID      iid;
    const char *label;
};

void *qi_any(IDeckLink *dl, const IidCandidate *cands, int n, char *matched) {
    for (int i = 0; i < n; i++) {
        void *p = nullptr;
        if (dl->QueryInterface(cands[i].iid, &p) == S_OK && p) {
            if (matched) std::snprintf(matched, 64, "%s", cands[i].label);
            return p;
        }
    }
    if (matched) std::snprintf(matched, 64, "none");
    return nullptr;
}

/* Cast to the CURRENT type is fine for GetFlag/GetInt — see the vtable note. */
IDeckLinkProfileAttributes *open_attrs(IDeckLink *dl) {
    static const IidCandidate c[] = {
        {IID_IDeckLinkProfileAttributes, "16.0"},
        {IID_IDeckLinkProfileAttributes_v15_3_1, "15.3.1"},
        {IID_IDeckLinkAttributes_v10_11, "10.11"},
    };
    return (IDeckLinkProfileAttributes *)qi_any(dl, c, 3, g_iface_attrs);
}

IDeckLinkStatus *open_status(IDeckLink *dl) {
    static const IidCandidate c[] = {
        {IID_IDeckLinkStatus, "16.0"},
        {IID_IDeckLinkStatus_v15_3_1, "15.3.1"},
    };
    return (IDeckLinkStatus *)qi_any(dl, c, 2, g_iface_status);
}

IDeckLinkOutput *open_output(IDeckLink *dl) {
    static const IidCandidate c[] = {
        {IID_IDeckLinkOutput, "16.0"},
        {IID_IDeckLinkOutput_v15_3_1, "15.3.1"},
        {IID_IDeckLinkOutput_v14_2_1, "14.2.1"},
        {IID_IDeckLinkOutput_v11_4, "11.4"},
        {IID_IDeckLinkOutput_v10_11, "10.11"},
    };
    return (IDeckLinkOutput *)qi_any(dl, c, 5, g_iface_output);
}

int64_t attr_int(IDeckLink *dl, BMDDeckLinkAttributeID id, int64_t dflt) {
    IDeckLinkProfileAttributes *a = open_attrs(dl);
    if (!a) return dflt;
    int64_t v = dflt;
    if (a->GetInt(id, &v) != S_OK) v = dflt;
    a->Release();
    return v;
}

bool attr_flag(IDeckLink *dl, BMDDeckLinkAttributeID id, bool dflt) {
    IDeckLinkProfileAttributes *a = open_attrs(dl);
    if (!a) return dflt;
    bool v = dflt;
    if (a->GetFlag(id, &v) != S_OK) v = dflt;
    a->Release();
    return v;
}

} /* namespace */

/* ------------------------------------------------------------------------ */

extern "C" {

int32_t pmg_dl_available(void) {
    /* True only if the framework loaded AND an iterator can be made. Having the
     * driver installed is not the same as it working. */
    IDeckLinkIterator *it = CreateDeckLinkIteratorInstance();
    if (!it) return 0;
    it->Release();
    return 1;
}

void pmg_dl_interface_report(char *buf, int32_t cap) {
    /* Which IID generation the DRIVER actually satisfied. If these read "none"
     * or lag the SDK badly, capabilities are being silently under-reported. */
    if (!buf || cap <= 0) return;
    std::snprintf(buf, static_cast<size_t>(cap), "attrs=%s status=%s output=%s",
                  g_iface_attrs, g_iface_status, g_iface_output);
}

void pmg_dl_sdk_version(char *buf, int32_t cap) {
    copy_into(buf, cap, BLACKMAGIC_DECKLINK_API_VERSION_STRING);
}

int32_t pmg_dl_device_count(void) {
    auto v = enumerate();
    int32_t n = static_cast<int32_t>(v.size());
    release_all(v);
    return n;
}

pmg_dl_result pmg_dl_device_info_at(int32_t index, pmg_dl_device_info *out) {
    if (!out) return PMG_DL_ERR_INTERNAL;
    std::memset(out, 0, sizeof(*out));

    auto v = enumerate();
    if (index < 0 || index >= static_cast<int32_t>(v.size())) {
        release_all(v);
        return PMG_DL_ERR_NO_DEVICE;
    }
    IDeckLink *dl = v[static_cast<size_t>(index)];

    CFStringRef s = nullptr;
    if (dl->GetDisplayName(&s) == S_OK) copy_into(out->display_name, sizeof(out->display_name), from_cf(s));
    s = nullptr;
    if (dl->GetModelName(&s) == S_OK) copy_into(out->model_name, sizeof(out->model_name), from_cf(s));

    out->index            = index;
    out->sub_device_count = static_cast<int32_t>(attr_int(dl, BMDDeckLinkNumberOfSubDevices, 0));
    out->sub_device_index = static_cast<int32_t>(attr_int(dl, BMDDeckLinkSubDeviceIndex, -1));

    /* PROBED, never assumed — rule 3. Model number does not predict either of
     * these, and external keying is the one that matters for fill+key. */
    out->supports_key_fill      = attr_flag(dl, BMDDeckLinkSupportsExternalKeying, false) ? 1 : 0;
    out->supports_internal_key  = attr_flag(dl, BMDDeckLinkSupportsInternalKeying, false) ? 1 : 0;

    /* persistent_id is PER CONNECTOR (measured: 0xC55080A0..A7 on a Quad 2).
     * topological_id is the physical CARD, and that is the one to group on —
     * a fill+key pair must be two connectors on one card, because one card is
     * one clock (ADR 0011). */
    out->persistent_id  = attr_int(dl, BMDDeckLinkPersistentID, 0);
    out->topological_id = attr_int(dl, BMDDeckLinkTopologicalID, 0);

    out->max_audio_channels = static_cast<int32_t>(attr_int(dl, BMDDeckLinkMaximumAudioChannels, 0));

    /* Reference input presence: ask the status interface. */
    IDeckLinkStatus *st = open_status(dl);
    if (st) {
        bool locked = false;
        out->has_reference_input = (st->GetFlag(bmdDeckLinkStatusReferenceSignalLocked, &locked) == S_OK) ? 1 : 0;
        st->Release();
    }

    release_all(v);
    return PMG_DL_OK;
}

pmg_dl_result pmg_dl_device_open(int32_t index, pmg_dl_device **out) {
    if (!out) return PMG_DL_ERR_INTERNAL;
    *out = nullptr;

    auto v = enumerate();
    if (index < 0 || index >= static_cast<int32_t>(v.size())) {
        release_all(v);
        return PMG_DL_ERR_NO_DEVICE;
    }
    Device *d = new Device();
    d->dl = v[static_cast<size_t>(index)];
    v[static_cast<size_t>(index)] = nullptr; /* ownership moves to Device */
    for (auto *x : v)
        if (x) x->Release();
    *out = reinterpret_cast<pmg_dl_device *>(d);
    return PMG_DL_OK;
}

void pmg_dl_device_release(pmg_dl_device *dev) {
    delete reinterpret_cast<Device *>(dev);
}

/* ---- Display modes ------------------------------------------------------ */

static pmg_dl_result with_modes(pmg_dl_device *dev, int32_t want, pmg_dl_mode *out, int32_t *count) {
    Device *d = reinterpret_cast<Device *>(dev);
    if (!d || !d->dl) return PMG_DL_ERR_NO_DEVICE;

    IDeckLinkOutput *o = open_output(d->dl);
    if (!o) return PMG_DL_ERR_NO_DEVICE;

    IDeckLinkDisplayModeIterator *it = nullptr;
    if (o->GetDisplayModeIterator(&it) != S_OK) {
        o->Release();
        return PMG_DL_ERR_INTERNAL;
    }

    int32_t i = 0;
    pmg_dl_result rc = PMG_DL_ERR_BAD_MODE;
    IDeckLinkDisplayMode *m = nullptr;
    while (it->Next(&m) == S_OK) {
        if (out && i == want) {
            std::memset(out, 0, sizeof(*out));
            out->width  = static_cast<int32_t>(m->GetWidth());
            out->height = static_cast<int32_t>(m->GetHeight());
            BMDTimeValue dur = 0;
            BMDTimeScale scale = 0;
            if (m->GetFrameRate(&dur, &scale) == S_OK && dur != 0) {
                /* Exact rational, always. 60000/1001, never 59.94. */
                out->fps_num = static_cast<int32_t>(scale);
                out->fps_den = static_cast<int32_t>(dur);
            }
            BMDFieldDominance fd = m->GetFieldDominance();
            out->interlaced = (fd == bmdLowerFieldFirst || fd == bmdUpperFieldFirst) ? 1 : 0;
            CFStringRef n = nullptr;
            if (m->GetName(&n) == S_OK) copy_into(out->name, sizeof(out->name), from_cf(n));
            rc = PMG_DL_OK;
        }
        m->Release();
        i++;
    }
    it->Release();
    o->Release();

    if (count) *count = i;
    return out ? rc : PMG_DL_OK;
}

int32_t pmg_dl_mode_count(pmg_dl_device *dev) {
    int32_t n = 0;
    if (with_modes(dev, -1, nullptr, &n) != PMG_DL_OK) return 0;
    return n;
}

pmg_dl_result pmg_dl_mode_at(pmg_dl_device *dev, int32_t i, pmg_dl_mode *out) {
    if (!out) return PMG_DL_ERR_INTERNAL;
    return with_modes(dev, i, out, nullptr);
}

/* ---- Reference ---------------------------------------------------------- */

pmg_dl_result pmg_dl_reference_status(pmg_dl_device *dev, pmg_dl_reference *out) {
    if (!out) return PMG_DL_ERR_INTERNAL;
    std::memset(out, 0, sizeof(*out));

    Device *d = reinterpret_cast<Device *>(dev);
    if (!d || !d->dl) return PMG_DL_ERR_NO_DEVICE;

    IDeckLinkStatus *st = open_status(d->dl);
    if (!st) return PMG_DL_ERR_INTERNAL;

    bool locked = false;
    out->present = (st->GetFlag(bmdDeckLinkStatusReferenceSignalLocked, &locked) == S_OK) ? 1 : 0;
    out->locked  = locked ? 1 : 0;

    if (locked) {
        int64_t mode = 0;
        if (st->GetInt(bmdDeckLinkStatusReferenceSignalMode, &mode) == S_OK) {
            /* BMDDisplayMode is itself a FourCC; report it as text so
             * INTEGRATION-NOTES records what the hardware actually said rather
             * than our interpretation of it. */
            char fourcc[5] = {0};
            fourcc[0] = static_cast<char>((mode >> 24) & 0xFF);
            fourcc[1] = static_cast<char>((mode >> 16) & 0xFF);
            fourcc[2] = static_cast<char>((mode >> 8) & 0xFF);
            fourcc[3] = static_cast<char>(mode & 0xFF);
            copy_into(out->format, sizeof(out->format), fourcc);
        }
    }
    st->Release();
    return PMG_DL_OK;
}

/* ---- Output: scheduled playback ----------------------------------------- */

} /* extern "C" */

namespace {

/*
 * A ring of completed-frame tokens.
 *
 * The completion callback runs on DeckLink's thread and MUST NOT allocate,
 * block, or take a lock (header rule 1). So it does exactly two things: bump
 * an atomic counter, and store an index. Single producer (DeckLink's thread),
 * single consumer (whoever calls pmg_dl_output_schedule).
 */
constexpr int kPoolSize = 8;

struct Output;

class Callback : public IDeckLinkVideoOutputCallback {
public:
    explicit Callback(Output *o) : m_out(o) {}

    /* IUnknown. This object is owned by Output, not by refcount — DeckLink is
     * given one reference and we outlive it deliberately, because destroying a
     * callback DeckLink might still be invoking is a race we do not want. */
    HRESULT QueryInterface(REFIID, void **) override { return E_NOINTERFACE; }
    ULONG AddRef() override { return ++m_refs; }
    ULONG Release() override { return --m_refs; }

    HRESULT ScheduledFrameCompleted(IDeckLinkVideoFrame *frame,
                                    BMDOutputFrameCompletionResult result) override;
    HRESULT ScheduledPlaybackHasStopped() override { return S_OK; }

private:
    Output          *m_out;
    std::atomic<ULONG> m_refs{1};
};

struct Output {
    IDeckLink       *dl      = nullptr;   /* borrowed from Device, not owned   */
    IDeckLinkOutput *out     = nullptr;
    Callback        *cb      = nullptr;

    IDeckLinkMutableVideoFrame *pool[kPoolSize] = {nullptr};
    /* Indices of frames DeckLink has handed back. Written only by the callback
     * thread, read only by the scheduling thread. */
    std::atomic<int>  free_ring[kPoolSize];
    std::atomic<uint64_t> free_head{0};   /* producer: callback thread         */
    std::atomic<uint64_t> free_tail{0};   /* consumer: scheduling thread       */

    int      width = 0, height = 0, row_bytes = 0;
    int64_t  frame_duration = 0, time_scale = 0;
    uint64_t next_display_time = 0;
    int      preroll = 0;
    bool     started = false;
    /* 0 when this output carries no audio, which is the common case for a
     * lower-third key feed and is not a fault. */
    int      audio_channels = 0;
    bool     audio_prerolling = false;
    std::atomic<uint64_t> n_audio{0};
    /* Last SDK failure, kept so a caller can report the vendor's own code
     * rather than our flattened one. */
    std::atomic<int64_t>  last_hr{0};
    const char *last_call = "";

    /* Counters. Written on the callback thread, read anywhere — relaxed is
     * fine, these are diagnostics, not control flow. */
    std::atomic<uint64_t> n_scheduled{0}, n_completed{0}, n_late{0}, n_dropped{0}, n_flushed{0};

    ~Output() {
        if (out) {
            out->StopScheduledPlayback(0, nullptr, 0);
            out->SetScheduledFrameCompletionCallback(nullptr);
            if (audio_channels > 0) out->DisableAudioOutput();
            out->DisableVideoOutput();
        }
        for (auto *&f : pool)
            if (f) { f->Release(); f = nullptr; }
        delete cb;
        if (out) out->Release();
    }
};

HRESULT Callback::ScheduledFrameCompleted(IDeckLinkVideoFrame *frame,
                                          BMDOutputFrameCompletionResult result) {
    /* ON DECKLINK'S THREAD. No allocation. No locks. No I/O. Nothing that can
     * block. Header rule 1 — one violation here is a late frame, and a late
     * frame is a visible glitch. */
    Output *o = m_out;

    switch (result) {
        case bmdOutputFrameCompleted:     o->n_completed.fetch_add(1, std::memory_order_relaxed); break;
        case bmdOutputFrameDisplayedLate: o->n_late.fetch_add(1, std::memory_order_relaxed);      break;
        case bmdOutputFrameDropped:       o->n_dropped.fetch_add(1, std::memory_order_relaxed);   break;
        case bmdOutputFrameFlushed:       o->n_flushed.fetch_add(1, std::memory_order_relaxed);   break;
        default: break;
    }

    /* Find which pool slot this was and publish it as free. Linear over 8
     * pointers — no allocation, no lock, and cheaper than a map lookup. */
    for (int i = 0; i < kPoolSize; i++) {
        if (o->pool[i] == frame) {
            uint64_t h = o->free_head.load(std::memory_order_relaxed);
            o->free_ring[h % kPoolSize].store(i, std::memory_order_relaxed);
            o->free_head.store(h + 1, std::memory_order_release);
            break;
        }
    }
    return S_OK;
}

/* Pop a free pool slot, or -1 if DeckLink has not returned one yet. */
int take_free_slot(Output *o) {
    uint64_t t = o->free_tail.load(std::memory_order_relaxed);
    if (t == o->free_head.load(std::memory_order_acquire)) return -1;
    int idx = o->free_ring[t % kPoolSize].load(std::memory_order_relaxed);
    o->free_tail.store(t + 1, std::memory_order_relaxed);
    return idx;
}

/* Resolve a BMDDisplayMode from the config's width/height/rational rate.
 * Matching on the enumerated modes rather than hard-coding a FourCC keeps this
 * honest about what the device actually reports. */
bool find_display_mode(IDeckLinkOutput *out, const pmg_dl_mode &want,
                       BMDDisplayMode *mode, int64_t *dur, int64_t *scale) {
    IDeckLinkDisplayModeIterator *it = nullptr;
    if (out->GetDisplayModeIterator(&it) != S_OK) return false;

    bool found = false;
    IDeckLinkDisplayMode *m = nullptr;
    while (it->Next(&m) == S_OK) {
        BMDTimeValue d = 0; BMDTimeScale s = 0;
        m->GetFrameRate(&d, &s);
        bool interlaced = (m->GetFieldDominance() == bmdLowerFieldFirst ||
                           m->GetFieldDominance() == bmdUpperFieldFirst);
        if (!found &&
            (int32_t)m->GetWidth()  == want.width &&
            (int32_t)m->GetHeight() == want.height &&
            (int32_t)s == want.fps_num && (int32_t)d == want.fps_den &&
            interlaced == (want.interlaced != 0)) {
            *mode = m->GetDisplayMode();
            *dur = d; *scale = s;
            found = true;
        }
        m->Release();
    }
    it->Release();
    return found;
}

} /* namespace */

extern "C" {

pmg_dl_result pmg_dl_output_open(pmg_dl_device *dev, const pmg_dl_output_config *cfg,
                                 pmg_dl_output **out_handle) {
    if (!out_handle || !cfg) return PMG_DL_ERR_INTERNAL;
    *out_handle = nullptr;

    Device *d = reinterpret_cast<Device *>(dev);
    if (!d || !d->dl) return PMG_DL_ERR_NO_DEVICE;

    /* External keying is PROBED. Refusing here, at open, beats discovering it
     * when a graphic fails to key on a Sunday. */
    if (cfg->key_mode == PMG_DL_KEY_FILL_KEY_PAIR &&
        !attr_flag(d->dl, BMDDeckLinkSupportsExternalKeying, false))
        return PMG_DL_ERR_NO_KEY_FILL;

    IDeckLinkOutput *o = open_output(d->dl);
    if (!o) return PMG_DL_ERR_NO_DEVICE;

    BMDDisplayMode mode = 0;
    int64_t dur = 0, scale = 0;
    if (!find_display_mode(o, cfg->mode, &mode, &dur, &scale)) {
        o->Release();
        return PMG_DL_ERR_BAD_MODE;
    }

    Output *op = new Output();
    op->dl = d->dl;
    op->out = o;
    op->width = cfg->mode.width;
    op->height = cfg->mode.height;
    /* v210 rows are padded to 48 pixels — DeckLink's own layout, and the one
     * pmg_color::v210 writes. "width / 6 * 16" is short for 1280 and shears
     * the picture. */
    const bool v210 = cfg->pixel_format == PMG_DL_PIXEL_V210;
    op->row_bytes = v210 ? ((cfg->mode.width + 47) / 48) * 128
                         : cfg->mode.width * 4;
    op->frame_duration = dur;
    op->time_scale = scale;
    op->preroll = cfg->preroll_frames > 0 ? cfg->preroll_frames : 3;
    if (op->preroll > kPoolSize - 1) op->preroll = kPoolSize - 1;

    if (o->EnableVideoOutput(mode, bmdVideoOutputFlagDefault) != S_OK) {
        delete op;
        return PMG_DL_ERR_BUSY;
    }

    /* Embedded audio, when the output is configured for it.
     *
     * CONTINUOUS, not timestamped. The card locks its audio clock to the video
     * clock it is already running; we only decide how many sample frames ride
     * with each video frame. That makes sync a property of the count, which is
     * exactly the property pmg_audio::AudioClock guarantees exactly.
     *
     * The channel count is fixed at open because DeckLink accepts only 2, 8 or
     * 16 — refusing an odd number here beats an E_INVALIDARG at the first
     * scheduled frame, which is on air. */
    if (cfg->audio_channels > 0) {
        if (cfg->audio_channels != 2 && cfg->audio_channels != 8 &&
            cfg->audio_channels != 16) {
            delete op;
            return PMG_DL_ERR_BAD_MODE;
        }
        if (o->EnableAudioOutput(bmdAudioSampleRate48kHz,
                                 bmdAudioSampleType32bitInteger,
                                 (uint32_t)cfg->audio_channels,
                                 bmdAudioOutputStreamContinuous) != S_OK) {
            delete op;
            return PMG_DL_ERR_BUSY;
        }
        op->audio_channels = cfg->audio_channels;

        /* Audio must be PREROLLED before it will accept a single sample.
         *
         * Measured 2026-08-15: without this, the first ScheduleAudioSamples
         * returns 0x80000009 (E_ACCESSDENIED) even though EnableAudioOutput
         * succeeded — the stream exists but is not open for business. The video
         * path needs no equivalent, which is exactly why it is easy to miss.
         *
         * The bracket is Begin -> schedule the preroll frames -> End ->
         * StartScheduledPlayback. */
        if (o->BeginAudioPreroll() != S_OK) {
            delete op;
            return PMG_DL_ERR_BUSY;
        }
        op->audio_prerolling = true;
    }

    /* Pre-create the whole pool. Allocating a frame mid-playback would be a
     * malloc on the critical path. */
    for (int i = 0; i < kPoolSize; i++) {
        if (o->CreateVideoFrame(op->width, op->height, op->row_bytes,
                                v210 ? bmdFormat10BitYUV : bmdFormat8BitBGRA,
                                bmdFrameFlagDefault,
                                &op->pool[i]) != S_OK) {
            delete op;
            return PMG_DL_ERR_INTERNAL;
        }
        /* Every slot starts free. */
        op->free_ring[i].store(i, std::memory_order_relaxed);
    }
    op->free_head.store(kPoolSize, std::memory_order_release);
    op->free_tail.store(0, std::memory_order_relaxed);

    op->cb = new Callback(op);
    o->SetScheduledFrameCompletionCallback(op->cb);

    *out_handle = reinterpret_cast<pmg_dl_output *>(op);
    return PMG_DL_OK;
}

void pmg_dl_output_release(pmg_dl_output *out) {
    delete reinterpret_cast<Output *>(out);
}

pmg_dl_result pmg_dl_output_set_callback(pmg_dl_output *, pmg_dl_frame_completed_cb, void *) {
    /* The shim owns completion handling: counters plus the free ring. Exposing
     * DeckLink's thread to Rust would make rule 1 Rust's problem to honour on
     * every call site, which is exactly what this boundary exists to prevent. */
    return PMG_DL_OK;
}

pmg_dl_result pmg_dl_output_schedule(pmg_dl_output *out,
                                     const void *fill, int32_t fill_stride,
                                     const void *key, int32_t /*key_stride*/,
                                     const void *audio, int32_t audio_samples,
                                     int64_t /*display_time*/, int64_t /*time_scale*/,
                                     uint64_t /*token*/) {
    Output *o = reinterpret_cast<Output *>(out);
    if (!o || !o->out || !fill) return PMG_DL_ERR_INTERNAL;
    if (key) return PMG_DL_ERR_NO_KEY_FILL;  /* fill+key pair is a later step */

    int slot = take_free_slot(o);
    if (slot < 0) return PMG_DL_ERR_BUSY;    /* caller should back off a frame */

    IDeckLinkMutableVideoFrame *f = o->pool[slot];

    /* Buffer access moved between SDK generations: 16.0 puts it on
     * IDeckLinkVideoBuffer (with StartAccess/EndAccess bracketing), older ones
     * had GetBytes directly on the frame.
     *
     * QUERY for whichever exists — never cast. A QueryInterface returns a
     * pointer genuinely valid for that interface; a cast across versions is
     * only valid inside a common vtable prefix, and frame interfaces do not
     * share one. This is the safe version of the fallback in open_attrs().  */
    void *dst = nullptr;
    IDeckLinkVideoBuffer *buf = nullptr;
    IDeckLinkVideoFrame_v14_2_1 *old = nullptr;

    if (f->QueryInterface(IID_IDeckLinkVideoBuffer, (void **)&buf) == S_OK && buf) {
        if (buf->StartAccess(bmdBufferAccessWrite) != S_OK || buf->GetBytes(&dst) != S_OK) {
            buf->Release();
            return PMG_DL_ERR_INTERNAL;
        }
    } else if (f->QueryInterface(IID_IDeckLinkVideoFrame_v14_2_1, (void **)&old) == S_OK && old) {
        if (old->GetBytes(&dst) != S_OK) {
            old->Release();
            return PMG_DL_ERR_INTERNAL;
        }
    } else {
        return PMG_DL_ERR_INTERNAL;
    }
    if (!dst) {
        if (buf) { buf->EndAccess(bmdBufferAccessWrite); buf->Release(); }
        if (old) old->Release();
        return PMG_DL_ERR_INTERNAL;
    }

    const uint8_t *src = static_cast<const uint8_t *>(fill);
    int32_t sstride = fill_stride > 0 ? fill_stride : o->row_bytes;
    if (sstride == o->row_bytes) {
        std::memcpy(dst, src, static_cast<size_t>(o->row_bytes) * o->height);
    } else {
        for (int y = 0; y < o->height; y++)
            std::memcpy(static_cast<uint8_t *>(dst) + (size_t)y * o->row_bytes,
                        src + (size_t)y * sstride, (size_t)o->row_bytes);
    }

    if (buf) { buf->EndAccess(bmdBufferAccessWrite); buf->Release(); }
    if (old) old->Release();

    /* Display times advance by exactly frame_duration in time_scale units —
     * rational throughout, never a float. */
    HRESULT hr = o->out->ScheduleVideoFrame(f,
                                            (BMDTimeValue)o->next_display_time,
                                            (BMDTimeValue)o->frame_duration,
                                            (BMDTimeScale)o->time_scale);
    if (hr != S_OK) {
        o->last_hr.store((int64_t)(uint32_t)hr, std::memory_order_relaxed);
        o->last_call = "ScheduleVideoFrame";
        return PMG_DL_ERR_INTERNAL;
    }

    o->next_display_time += (uint64_t)o->frame_duration;
    o->n_scheduled.fetch_add(1, std::memory_order_relaxed);

    /* Audio AFTER the video frame is scheduled, so a video failure above never
     * leaves the card holding samples for a picture that will not arrive —
     * which would offset every frame after it by this frame's worth. */
    if (audio && audio_samples > 0 && o->audio_channels > 0) {
        uint32_t written = 0;
        HRESULT ahr = o->out->ScheduleAudioSamples(const_cast<void *>(audio),
                                                   (uint32_t)audio_samples,
                                                   0, 0, &written);
        if (ahr != S_OK) {
            o->last_hr.store((int64_t)(uint32_t)ahr, std::memory_order_relaxed);
            o->last_call = "ScheduleAudioSamples";
            return PMG_DL_ERR_INTERNAL;
        }
        /* A SHORT WRITE IS NOT AN ERROR TO THE API and is a permanent sync
         * offset to a viewer: the card took fewer samples than this frame owns,
         * and every frame after it is early by the difference. Counted as what
         * was actually accepted so `buffered_audio` and the caller's own clock
         * disagree visibly rather than quietly. */
        o->n_audio.fetch_add(written, std::memory_order_relaxed);
    }

    /* Start only once enough frames are queued. Starting on frame 1 underruns
     * immediately; preroll depth is what spike S3 measures. */
    if (!o->started && o->n_scheduled.load(std::memory_order_relaxed) >= (uint64_t)o->preroll) {
        /* Close the audio preroll bracket FIRST. Starting playback with the
         * stream still prerolling leaves the card holding samples it will not
         * play, and the video runs away from audio that never starts. */
        if (o->audio_prerolling) {
            HRESULT ehr = o->out->EndAudioPreroll();
            if (ehr != S_OK) {
                o->last_hr.store((int64_t)(uint32_t)ehr, std::memory_order_relaxed);
                o->last_call = "EndAudioPreroll";
                return PMG_DL_ERR_INTERNAL;
            }
            o->audio_prerolling = false;
        }
        if (o->out->StartScheduledPlayback(0, (BMDTimeScale)o->time_scale, 1.0) == S_OK)
            o->started = true;
    }
    return PMG_DL_OK;
}

pmg_dl_result pmg_dl_output_start(pmg_dl_output *out) {
    Output *o = reinterpret_cast<Output *>(out);
    if (!o || !o->out) return PMG_DL_ERR_INTERNAL;
    if (o->started) return PMG_DL_OK;
    if (o->out->StartScheduledPlayback(0, (BMDTimeScale)o->time_scale, 1.0) != S_OK)
        return PMG_DL_ERR_INTERNAL;
    o->started = true;
    return PMG_DL_OK;
}

pmg_dl_result pmg_dl_output_stop(pmg_dl_output *out) {
    Output *o = reinterpret_cast<Output *>(out);
    if (!o || !o->out) return PMG_DL_ERR_INTERNAL;
    BMDTimeValue actual = 0;
    o->out->StopScheduledPlayback(0, &actual, (BMDTimeScale)o->time_scale);
    o->started = false;
    return PMG_DL_OK;
}

pmg_dl_result pmg_dl_output_stats(pmg_dl_output *out, pmg_dl_stats *stats) {
    Output *o = reinterpret_cast<Output *>(out);
    if (!o || !stats) return PMG_DL_ERR_INTERNAL;
    stats->scheduled      = o->n_scheduled.load(std::memory_order_relaxed);
    stats->completed      = o->n_completed.load(std::memory_order_relaxed);
    stats->displayed_late = o->n_late.load(std::memory_order_relaxed);
    stats->dropped        = o->n_dropped.load(std::memory_order_relaxed);
    uint32_t buffered = 0;
    if (o->out) o->out->GetBufferedVideoFrameCount(&buffered);
    stats->buffered = buffered;

    uint32_t abuf = 0;
    if (o->out && o->audio_channels > 0)
        o->out->GetBufferedAudioSampleFrameCount(&abuf);
    stats->buffered_audio  = abuf;
    stats->audio_scheduled = o->n_audio.load(std::memory_order_relaxed);
    return PMG_DL_OK;
}

int64_t pmg_dl_output_last_hresult(pmg_dl_output *out) {
    Output *o = reinterpret_cast<Output *>(out);
    return o ? o->last_hr.load(std::memory_order_relaxed) : 0;
}

const char *pmg_dl_output_last_call(pmg_dl_output *out) {
    Output *o = reinterpret_cast<Output *>(out);
    return (o && o->last_call) ? o->last_call : "";
}
} /* extern "C" */
