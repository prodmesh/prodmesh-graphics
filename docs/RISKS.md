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

**Status:** open · **Spike:** [S1](spikes/S1-render-parity.md) · **ADRs:** 0001, 0002

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

**Status:** open · **Spike:** [S2](spikes/S2-text-typography.md)

**Recommended stack:** `parley` + `swash` + `fontique`, with `harfrust`/
`rustybuzz` underneath. `cosmic-text` is the pragmatic all-in-one and it works,
but its API is text-editor-shaped, its line breaking is simplistic, and it bakes
in a system font database — all three fight what we need.

What will actually break, in order of how much it will hurt:

- **Subpixel positioning.** Broadcast CG slides text horizontally during
  animation. Integer-snapped glyphs *visibly shudder*. Needs fractional-offset
  glyph caching (quantise to ⅛ px, cache per phase) or SDF glyphs. This is the
  single most likely thing to make the output look amateur.
- **Antialiasing over a key.** Grayscale only — subpixel-RGB AA is meaningless
  over an alpha channel. The AA edge must be premultiplied correctly or you get
  dark or light fringes when keyed over live video. This is *the* classic
  broadcast CG bug. Test over mid-grey and over saturated backgrounds explicitly.
- **OpenType features.** `kern`, `liga`, `smcp`, `ss01`, and especially `tnum`
  (tabular figures — clocks and scores jitter without it). Per-run feature
  control belongs in the scene format.
- **Vertical metrics.** The OS/2 typo vs hhea vs win mess. Pick one metric set,
  document it, and **never change it** — changing it moves every existing
  template by two pixels.
- **Emoji.** COLRv1 via swash is fine. Declare sbix and SVG-in-OpenType
  unsupported rather than half-supporting them.
- **RTL and CJK.** Shaping is handled by the stack; the *layout* side is the
  work — bidi runs (`unicode-bidi`) and line breaking (`icu_segmenter`). CJK line
  breaking without ICU is wrong in ways Korean and Japanese readers notice
  immediately.

---

## R4 — DeckLink from Rust is a C++ COM-alike, not a C API

**Status:** partially confirmed (2026-08-14) — enumeration and capability probing
work through the hand-written shim against a real Quad 2; the output path is
untouched · **Spike:** [S3](spikes/S3-decklink-output.md) · **ADR:** 0003

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
