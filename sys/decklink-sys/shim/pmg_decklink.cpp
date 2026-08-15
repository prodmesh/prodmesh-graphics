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

/* ---- Output: spike S3 ---------------------------------------------------
 *
 * Deliberately unimplemented. Scheduled playback is where the real risk lives
 * (rule 1: the completion callback must not allocate, block, or lock), and it
 * gets built with the ten-minute zero-late-frame soak in
 * docs/spikes/S3-decklink-output.md rather than in passing.
 */

pmg_dl_result pmg_dl_output_open(pmg_dl_device *, const pmg_dl_output_config *, pmg_dl_output **out) {
    if (out) *out = nullptr;
    return PMG_DL_ERR_INTERNAL;
}
void          pmg_dl_output_release(pmg_dl_output *) {}
pmg_dl_result pmg_dl_output_set_callback(pmg_dl_output *, pmg_dl_frame_completed_cb, void *) { return PMG_DL_ERR_INTERNAL; }
pmg_dl_result pmg_dl_output_schedule(pmg_dl_output *, const void *, int32_t, const void *, int32_t,
                                     const void *, int32_t, int64_t, int64_t, uint64_t) { return PMG_DL_ERR_INTERNAL; }
pmg_dl_result pmg_dl_output_start(pmg_dl_output *) { return PMG_DL_ERR_INTERNAL; }
pmg_dl_result pmg_dl_output_stop(pmg_dl_output *) { return PMG_DL_ERR_INTERNAL; }
pmg_dl_result pmg_dl_output_stats(pmg_dl_output *, pmg_dl_stats *) { return PMG_DL_ERR_INTERNAL; }

} /* extern "C" */
