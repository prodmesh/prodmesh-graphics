# Risk register

Living document. Each risk carries a current belief, a mitigation, the spike that
will tell us whether the belief is right, and a status. When a spike returns, its
findings land in `docs/INTEGRATION-NOTES.md` — dated and hardware-attributed —
and the belief here gets updated to match.

**Status legend:** `open` (unverified belief) · `spiking` · `confirmed` ·
`refuted` (belief was wrong; linked ADR needs revisiting) · `accepted` (real, not
going to fix).

---

## R1 — Native and browser rendering will not be bit-identical

**Status:** **refuted on Metal** (2026-08-14) — they are bit-identical, 0/255 max
delta and zero geometric displacement across the whole corpus. **Still open for
cross-backend**: Chrome was targeting the same Metal driver, so D3D12 and Vulkan
remain untested ·
**Spike:** [S1](spikes/S1-render-parity.md) · **ADRs:** 0001, 0002

> **The belief in this risk was wrong, in the good direction — but read the
> caveat.** The measurement was Dawn-on-Metal against wgpu-on-Metal, i.e. two
> WGSL frontends over one driver. That is a genuine result (it proves nothing in
> the pipeline depends on backend-defined behaviour) and it is *not* the
> cross-backend claim. Quote ADR 0001's promise as **measured on Metal** until
> the corpus runs on Windows.

ADR 0001 rests on "one renderer, so the designer and the node agree." That is
true of the code and not automatically true of the pixels. Native wgpu goes to
Metal; the browser goes to the engine's own WebGPU implementation. They differ in
MSAA sample patterns, fragment-shader float precision, sRGB and gamma handling in
the swapchain and canvas compositor, and available limits.

**Reframe the promise honestly.** Shared code guarantees identical *layout,
glyph rasterization, binding evaluation, and timeline math*. That is where the
bugs actually live, and it eliminates the entire class of "the text wrapped
differently in the designer." Colour and antialiasing may differ in the least
significant bits. Saying so now is better than discovering it in M4.

**Mitigations:** render offscreen to RGBA8 at design resolution on both sides; do
**all** antialiasing yourself, analytically or with deterministic ordered
supersampling; **never enable MSAA**; never rely on filtering defaults; pin one
wgpu version across both targets and treat upgrading it as a release-gated change
with a full golden re-run. Canvas presentation is then the only uncontrolled step.

**Bar:** ≥99.9% of pixels within 1/255, max channel delta ≤2/255, and **zero**
geometric displacement. A shifted glyph is a different class of failure from a
shade difference and is reported separately.

---

## R2 — Genlock gives phase, not frame numbering

**Status:** confirmed by construction · **Spike:** [S4](spikes/S4-genlock-phase.md) · **ADR:** 0011

Two nodes locked to the same blackburst emit frames in lockstep. They do **not**
agree on which frame is number 1841944. Someone will assume a shared reference
means a shared frame count and design a cross-node simultaneous take on top of
it.

| Scope | What locks it | v1 promise |
|---|---|---|
| Outputs on one DeckLink device | Same scheduled display time, shared device clock | **Yes** |
| Two cards, one machine | Reference to both from a DA | Yes, with the wiring |
| Across machines | Same reference to every node | Frame *timing* aligns only |
| GPU display output | Nothing, absent a Quadro Sync card | **No** |

**Mitigation:** say it in ADR 0011 and in `AGENTS.md`. Ship the `at:` field on
`Cue` from day one — unused in v1, but free now and a protocol break later.
Frame-accurate cross-node takes need timecode from the same generator, and that
is an M6-or-later conversation.

---

## R3 — Rust text quality is the thing most likely to look amateur

**Status:** **partly refuted, partly confirmed** (2026-08-15) ·
**Spike:** [S2](spikes/S2-text-typography.md) — **run, and it failed on shudder**

> **S2 changed what this risk is about.** Fringing, tabular figures, and glyph
> cache cost all measure clean. But there IS a shudder — **0.33 px for text,
> 0.21 px for a plain box** — and the control proves it is **not a text problem
> at all**: an antialiased box with no texture sampling stutters the same way.
>
> R3 frames shudder as text-specific and solved by sub-pixel glyph caching. The
> measurement says sub-pixel caching governs *within-run glyph placement*, while
> *motion smoothness* is a property of the coverage function and the sampler —
> two different mechanisms. **Root cause is open**; smoothstep-vs-linear coverage
> was tested and disproved. Whether 0.2 px is visible still needs eyes.

`pmg-text` is built on **swash** — shaping *and* rasterization, with explicit
subpixel-offset control, which is the reason to pick it. `parley` is deferred to
M5, when multi-line, bidi, and rich text actually arrive; `TextMeasure` is a
trait so that swap costs nothing.

Each of the three hazards below now has a structural answer and a test. What is
*not* yet established is quality — whether ⅛ px is enough phases, whether the
edges look right keyed over live video. That is still spike S2's job, and it
needs eyes on real hardware, not assertions.

**Found while building it, and worth keeping:** metrics came back in font
**design units** — an ascent of 85,284 "px" at 46 px — because `linear_scale()`
multiplies raw units while `scale(ppem)` divides by units-per-em first. Every
relationship assertion passed anyway: positive, monotonic, scaling linearly,
because *both* numbers were wrong by the same factor. A suite that only compares
values to each other cannot catch a units error, so the metrics test now carries
**absolute** plausibility bounds as well.

What will actually break, in order of how much it will hurt:

- **Subpixel positioning.** ✅ **Implemented:** glyphs rasterize at 8 fractional
  offsets and the phase is part of the cache key. Measured on Arial at 46 px, an
  `M` across the phases varies in width (32↔33 px) and left bearing (3→4) while
  its coverage sum holds at ~128,240 ± 20 — the same ink redistributed across
  pixel boundaries, which is exactly what stops the shudder. A test asserts
  phases 0 and 4 do **not** rasterize identically; without it, a regression here
  would be invisible in a still frame and obvious in motion.
- **Antialiasing over a key.** ✅ **Structurally enforced:** the atlas stores a
  **single coverage channel** and nothing else — never premultiplied, never
  coloured, never subpixel-RGB. Premultiplication happens once, at composite, in
  the renderer's linear space (ADR 0012). Baking colour into the atlas would
  premultiply in the wrong space *and* make the cache per-colour. Hinting is also
  off: grid-fitting snaps glyphs to whole pixels, which is the shudder above.
  **Still to verify by eye** over mid-grey and saturated backgrounds — S2.
- **OpenType features.** `kern`, `liga`, `smcp`, `ss01`, and especially `tnum`
  (tabular figures — clocks and scores jitter without it). Per-run feature
  control belongs in the scene format.
- **Vertical metrics.** ✅ **Decided:** OS/2 typographic metrics when the font
  sets `USE_TYPO_METRICS` (fsSelection bit 7), otherwise hhea — swash's rule,
  and the rule browsers use, so text sits where a designer expects. It reads as
  conditional but **cannot drift**: fonts are content-addressed, so a given font
  hash always carries the same flag and therefore the same baseline. Documented
  on `pmg_text::Metrics`. **Never change it** — doing so moves every existing
  template by a couple of pixels, silently.
- **Emoji.** COLRv1 via swash is fine. Declare sbix and SVG-in-OpenType
  unsupported rather than half-supporting them.
- **RTL and CJK.** Shaping is handled by the stack; the *layout* side is the
  work — bidi runs (`unicode-bidi`) and line breaking (`icu_segmenter`). CJK line
  breaking without ICU is wrong in ways Korean and Japanese readers notice
  immediately.

---

## R4 — DeckLink from Rust is a C++ COM-alike, not a C API

**Status:** **largely retired** (2026-08-14) — the shim sustains 1080p59.94 for
ten minutes with zero late and zero dropped frames on a real Quad 2. ADR 0003 is
validated for output. What remains is the reference leg and scope confirmation of
a fill+key pair, both blocked on a generator ·
**Spike:** [S3](spikes/S3-decklink-output.md) · **ADR:** 0003

> **Found in the doing, and worse than the risk as originally written:** the SDK
> and the installed *driver* are different versions, and a newer SDK's IIDs are
> simply absent from an older driver. `QueryInterface` fails and every capability
> reads as its default — the card looks incapable rather than the query looking
> broken. Fixed with a newest-first IID fallback chain. Full detail, including
> the vtable-slot constraint that makes the chain safe and where it stops being
> safe, in `docs/INTEGRATION-NOTES.md`.

The SDK is COM on Windows and a hand-rolled COM-alike on macOS and Linux:
`IUnknown` refcounting, `QueryInterface`, and virtual interfaces such as
`IDeckLinkVideoOutputCallback` that you must *implement*. Crates exist on
crates.io; they are thin, incomplete, and none covers scheduled playback plus
reference plus key/fill plus embedded audio to the standard needed here.

**Plan to own a shim, not to depend on a crate.** A small C++ translation unit in
`sys/decklink-sys/shim/` exposing a flat `extern "C"` API, compiled by the `cc`
crate, bound **by hand against a hand-written header that is the contract of
record**. Never point bindgen at C++ virtual interfaces. This is exactly the
prodmesh-caption Swift-shim pattern (`swift/include/pmcaption_speech.h`) and the
convention is reused verbatim, comment included.

Specific hazards: the completion callback arrives on DeckLink's own thread —
never allocate, never block, never take a lock there, just push a token into a
bounded channel. Every interface pointer needs a Rust wrapper whose `Drop` calls
`Release`. At 1080p59.94 the budget is 16.68 ms to lay out, render, read back,
and hand over; one underrun is a visible glitch plus a
`bmdOutputFrameDisplayedLate`. For key+fill, the reliable path is **two ports on
the same device scheduled at the same display time** — same device means same
clock, which is what actually guarantees phase. Device capability varies wildly,
so probing is mandatory.

---

## R5 — Frame-accurate video decode

**Status:** open · related measurement: a CPU generate-and-encode path at 1080p
took 17.55 ms against a 16.68 ms budget (2026-08-14), so the decode-to-output
path has no headroom for a software copy either · **Spike:** [S5](spikes/S5-media-decode-sync.md)

**Recommendation:** ffmpeg via `rsmpeg`/`ffmpeg-next` for demux and decode, with
platform hardware acceleration where available. `symphonia` is genuinely good for
audio. Pure-Rust H.264/HEVC is not production-ready.

**The decision that removes most of the risk: require an all-intra intermediate
for any media that must hit a mark.** ProRes 4444, DNxHR, or HAP with alpha —
frame-accurately seekable by construction, and they carry the alpha channel keyed
content needs anyway. The control server transcodes on ingest. Long-GOP H.264
with B-frames and open GOPs is where frame-accurate seek goes to die; accept it
for convenience playback, transcode it for anything cued.

**The clock is always the video output.** Audio resamples to fit. The decoder is
*pulled* by the output callback, never pushed. On decoder underrun: hold the last
frame, log it, **never stall the output**.

Also: variable-frame-rate sources are **rejected at ingest**, not accommodated.

**Licensing seam:** LGPL ffmpeg components only, dynamically linked, no GPL. This
is a productization seam and must be written down, not assumed — see ADR 0003.

---

## R6 — WebGPU in the browser, single-threaded

**Status:** open · **Spike:** [S6](spikes/S6-webgpu-browser.md)

Support is real as of mid-2026 across Chrome/Edge, Safari 26, and Firefox.
**Recommendation: require Chrome or Edge for Design mode, Safari best-effort, and
say so in the UI.** Configure, Edit, and Show are ordinary DOM and must work
anywhere.

- **Limits are lower than native.** Write shaders to the WebGPU downlevel tier
  and validate against those limits in CI, not against the dev machine's.
- **Fonts must be loaded as bytes from the CAS and handed to `pmg-text`.** Never
  use CSS fonts for canvas text — that is the parity killer.
- **`SharedArrayBuffer` and WASM threads need COOP/COEP**, which conflicts with a
  plain-HTTP LAN appliance with no TLS. So **plan for single-threaded WASM** and
  size the work accordingly: designer preview at half resolution and 30 fps is
  fine. Do not attempt a full 1080p59.94 preview in a tab.
- Bundle size will be several megabytes. Fine on a LAN, and the no-CDN rule means
  it is served locally anyway.

---

## R7 — Preview transport

**Status:** open · **Spike:** [S7](spikes/S7-preview-transport.md)

| Option | Latency | Verdict |
|---|---|---|
| MJPEG multipart | ~1 frame | Trivial and works everywhere. Burns a browser connection per stream — the six-per-origin cap again, which is exactly the lesson ADR 0010 taught ProdMesh-Control. **Fine for M2.** |
| **WebCodecs + H.264 over WebSocket** | very low | **The answer.** One connection, hardware encode, all-intra or short GOP so joiners paint instantly, `VideoDecoder` to canvas. No SDP, no ICE, no TURN, no TLS requirement. **M3.** |
| WebRTC | lowest | Signalling, ICE, and an SFU question at eight viewers. Defer to P2, probably never. |

**Non-negotiable:** preview downscales *the same GPU texture program uses*, reads
back asynchronously (never a blocking map), on a lower-priority queue with a hard
budget. **If preview exceeds its budget, drop the preview, log it, keep the show
on air.** Preview never shares the control socket.

---

## R8 — Cue delivery under failure

**Status:** open · **Spike:** [S8](spikes/S8-cue-chaos.md) · **ADRs:** 0007, 0008

The honest analysis: over TCP, packet loss is not the failure mode. **Connection
loss and slow applications are.** Therefore:

- `seq` and acks earn their keep across *reconnects* and for idempotency — not
  for reordering. Say so, so nobody adds machinery for a problem TCP solved.
- The real hazard is a cue queued behind a large message. **Assets over HTTP,
  telemetry small, preview on its own socket** — that framing decision *is* the
  mitigation.
- A WebSocket over dying wifi can take 30+ seconds to fail, so a 1 s / 3 s
  application heartbeat greys the UI fast, and deadline rejection kills zombie
  cues that arrive after a long reconnect.
- **The failure nobody designs for is the operator's own duplicate** — a
  double-click, or a retry after a UI timeout. `cue_id` generated in the browser
  and carried end to end.

---

## R9 — Colour, range, and the key channel

**Status:** open · **Spike:** [S9](spikes/S9-color-measurement.md) · **ADR:** 0012

In descending order of how likely each is to actually happen:

1. **Compositing in gamma-encoded values** → dark halos on white text over a dark
   key. Render in linear float, premultiplied; encode once at output.
2. **The matrix problem.** Fill goes RGB→YCbCr through 601 (SD), 709 (HD), or
   2020 (UHD). **The key is a monotone alpha signal, not a colour, and must not
   go through a colour matrix.** Write `Y = alpha` mapped to the configured
   range, `Cb = Cr =` neutral (512 in 10-bit). Getting this wrong makes the key
   subtly wrong at the edges and the downstream keyer's soft edges wrong with it.
3. **Legal vs full range.** Switchers expect narrow (64–940 in 10-bit); some
   plants want a full-range key. Per-output configurable, and **provable in
   Configure with a ramp test pattern with marked 0% and 100% points**.
4. **Premultiplied vs straight alpha.** DeckLink's embedded-alpha modes and NDI
   each have an opinion. Internally premultiplied; convert at the boundary;
   document which and where.
5. **HDR: don't.** v1 is SDR 709. But name primaries, transfer, matrix, and range
   explicitly in the output config struct so HDR is additive later, and **reject
   HDR-tagged source media at ingest with a clear message** rather than mangling
   it.
6. **Media carries its own colour tags.** Convert on decode. Never assume 709.

---

## R10 — Font licensing on export

**Status:** accepted, seam recorded · **ADR:** 0005

Embedding font bytes in a bundle that ships to nodes is redistribution. Fine
in-house; not fine productized. The seam is one field on the export bundle
format: `include_fonts: bool`, versus reference-by-identity with a
require-local-install check. One field now; a format break later.

---

## R11 — wgpu version churn

**Status:** accepted · **ADR:** 0002

wgpu makes breaking changes regularly, and we are pinned to one version across
two targets that must agree. **Treat a wgpu upgrade as a release-gated change
requiring a full golden-image re-run**, never as a routine dependency bump.

---

## R12 — Scope creep from the ProPresenter-subsumption goal

**Status:** accepted, managed by ADR 0015 and `docs/VISION.md`

The data model is designed so screen content *could* move here. The temptation
will be to start rendering lyrics early because the model supports it. **A CG
glitch is embarrassing; a lyrics glitch stops worship.** Different failure
budgets, and mixing them early means the strict one sets the pace for everything
else. Keep subsumption in the data model and out of the renderer scope until M6
has an honest evaluation of it.
