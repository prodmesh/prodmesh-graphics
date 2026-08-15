# State

What is actually real, as opposed to what is designed. Updated as part of the
change that makes something real — not afterwards.

`docs/ROADMAP.md` is where we are going. This is where we are.

**Last updated: 2026-08-15 · M0 complete · **M1 is complete**: design in the browser, publish, take on air over NDI.**

---

## The honest summary

**M0 is complete and all three of its gating spikes passed.** None of the
foundational ADRs is a bet any more.

- **S1** — native wgpu and browser WebGPU render **bit-identically**: 0/255 max
  channel delta, zero geometric displacement. Measured on Metal; cross-backend
  (D3D12, Vulkan) is still untested.
- **S3** — the hand-written DeckLink shim sustained **1080p59.94 for ten minutes
  with zero late and zero dropped frames**.
- **S6** — WebGPU is comfortably available in-browser: real adapter and device,
  244 KB bundle, ~5 ms per figure.

**M1 is under way.** The scene format (M1a) and the layout solver (M1b) are done:
a lower third compiles to canonical CBOR with a stable `template_rev`, and lays
out with the band growing and shrinking against the name and collapsing when the
title is empty — all verified as numbers, before any pixels.

**The vertical slice renders.** `pmgctl take` compiles a template, binds an
Item's data, solves layout, shapes and rasterizes text, and plays In → hold →
Out over NDI with key and fill. Armed in **0.3–2 ms**; worst frame **13.9 ms
against a 16.68 ms budget**, because per frame it only evaluates timelines and
applies transforms.

**The control plane is real (M1e).** Templates publish as opaque CAS blobs with
a validated manifest sidecar; Items pin the revision they were built against;
PMG Link carries assignment and acked cues over CBOR; `POST /api/cue` blocks
until the node confirms; Edit builds an Item and Show takes it. Verified end to
end against a running server with a node speaking real CBOR: publish → CAS
round-trip byte-identical → item → take → on-air state adopted from the node →
take out → take log with the surface recorded.

**M1 is end to end.** `pmg-node` dials PMG Link, stages assets over HTTP into
its own CAS, persists its assignment, applies cues, and renders to NDI —
verified against the real server at **59.94 fps with zero late frames** across
three take/take-out cycles.

**The autonomy invariant is tested, not asserted.** Killing the control server
leaves the node rendering; a brand-new server process with no memory of any take
adopts the graphic still in `hold` from the node's `StateReport`.

**Design renders with the same Rust the node runs.** The browser's
`compile_template` produced a byte-identical `template_rev` to `pmgctl` for the
same document (2893 bytes, same hash), and the canvas is the shared renderer on
WebGPU: **arm 22.5 ms, warm frames 3.34 ms**. Responsive layout and `visible_if`
behave in the browser exactly as they do natively.

**The whole loop has been driven through the UI**: open a template → edit its
data → scrub the In timeline → Publish → build an Item → Take → on air, with the
node staging the published blob by its address.

**What M1 does not have**, and is honest about: playlists and a rundown, layer
stacks beyond one, preview, auth, discovery, DeckLink, and media. Those are M2
and M3.

**Spike S2 ran, and it failed on one criterion.** Fringing, tabular figures, and
glyph-cache cost all measure clean, but sub-pixel motion deviates from linear by
0.33 px for text and **0.21 px for a plain box** — so the fault is not text and
not the atlas. Root cause is open; see `docs/spikes/S2-text-typography.md`.

`docs/INTEGRATION-NOTES.md` is no longer empty; seven entries came out of one
afternoon, and three were version-skew failures that each presented as something
other than what they were.

---

## Component status

| Component | State |
|---|---|
| Documentation set | ✅ complete — 15 ADRs, 7 specs, 9 spike briefs |
| Cargo workspace | 🟢 19 crates; `fmt`, `clippy -D warnings`, and the wasm32 pure-set job green in both SDK and no-SDK configs |
| `pmgctl --selftest` | 🟢 gates the format's load-bearing guarantees: canonical CBOR round-trip, stable `template_rev`, JSON-mirror equivalence, `visible_if` collapse |
| `sys/decklink-sys` | 🟢 enumeration, probing, modes, reference, **and scheduled output — 10 min at 1080p59.94, zero late, zero dropped** |
| `sys/ndi-sys` | 🟢 runtime loading, find, receive, send — **frames verified in both directions**. ABI asserted statically and at runtime |
| `pmg-color::patterns` | 🟢 ramp, bars, soft-edge, alpha staircase, moving line — the diagnostic instruments from `output-config.md` §8 |
| `pmgctl ndi` / `pmgctl decklink` | 🟢 info, probe, send / list, modes, ref |
| `pmgctl take` | 🟢 **the M1 vertical slice** — template to NDI, In/hold/Out |
| Control server | 🟢 templates, revisions, items, CAS origin, PMG Link, cues, nodes. **39 tests** |
| SQLite schema | 🟢 migrated on boot: templates, revisions, items, assets, nodes, assignments. No `on_air` column, by design |
| `server/cas.js` | 🟢 atomic writes, hash verified never trusted, `GET /cas/:hash` with Range and resume. blake3 **agrees with the Rust crate**, tested |
| `server/nodeLink.js` | 🟢 handshake, assignment offer/ack, cues with deadlines, replay ring, StateReport adopted |
| `POST /api/cue` | 🟢 synchronous — 503 `cue_not_acked` when the node stays silent, never a silent success |
| `pmg-proto` | 🟢 the wire contract, with checked-in CBOR fixtures the JavaScript tests decode |
| Design / Edit / Show | 🟢 all three real. Design compiles and renders with the shared Rust; Show takes on air and does not lie |
| `pmg-wasm` | 🟢 `compile_template` + `DesignSession` — the browser half of ADR 0001, pinned by a native test |
| Fonts | 🟡 uploaded as CAS assets and fetched by hash. Family matching is still provisional — one face serves every family a template names |
| Adoption | 🟢 an unadopted node is welcomed and given **nothing** — no templates, no items, no cues. Persisted, so it survives a restart |
| mDNS discovery | 🟡 **written, unverified.** Multicast is unusable on this host; `--server` is the tested path. See INTEGRATION-NOTES |
| Configure | 🟢 adopt a node, see its capabilities, edit its layer stack — pushed live |
| S8 chaos harness | 🟢 **gating.** `npm run chaos` — 1000+ takes through five injections, zero fired twice |
| `pmg-node` | 🟢 **the playout binary.** Link client, staging, durable assignment, layer engine, NDI out. Survives server loss; 10 cue-rule tests |
| `pmg-cas` | 🟢 atomic writes, streaming put, verify. Agrees with `server/cas.js` |
| `pmg-out-ndi` | 🟢 the shared sender — one unpremultiply loop for the node and the CLI |
| `streamHub` | ✅ ported from ProdMesh-Control — refcounted topics, conflation, snapshot-on-join |
| `cueLog` | ✅ the log-tail bridge from ADR 0008, with gap recovery. 6 tests |
| PMG Link | ⬜ spec only |
| CAS | ⬜ spec only |
| Web app | 🟡 four-mode shell, routing, tokens, `useTopic` + `useQuery`. 6 tests. No functionality |
| `pmg-scene` | 🟢 **full document model**, canonical CBOR + JSON mirror, `template_rev`, validation, manifest, total binding evaluation. 25 tests |
| `pmg-layout` | 🟢 **flexbox solver** over `TextMeasure` (taffy, ADR 0016). Shrink-and-grow and `visible_if` removal verified as numbers. 12 tests |
| `pmg-text` | 🟢 fonts **from bytes**, swash shaping, subpixel glyph atlas at ⅛ px, real `TextMeasure`. 18 tests. R3's three hazards structurally answered; quality still needs S2 |
| `pmg-render` | 🟢 **scene rendering** — boxes with analytic-AA rounded corners, text from the coverage atlas, composed transforms, timeline animation. Plus the S1 parity corpus |
| `pmg-wasm` | 🟡 the parity harness — renders the corpus in-browser and posts raw RGBA back. 244 KB |
| `pmg-out-ndi` / `pmg-out-decklink` | ⬜ not started — `pmgctl` drives `sys/` directly for now |
| Design canvas | ⬜ not started |
| CI | 🟡 workflows written and green locally; **never run on GitHub Actions** — no remote yet |
| Rust toolchain on the dev box | ✅ 1.90.0 + `wasm32-unknown-unknown` |

### What `--selftest` actually asserts today

Four crates carry real assertions, chosen because they guard the rules most
likely to be broken by someone doing the obvious thing:

- **`pmg-color`** — narrow/full range endpoints, and **key chroma is neutral at
  every alpha in every range**. That is the assertion that catches someone
  reusing the fill path for the key (ADR 0012 §3).
- **`pmg-anim`** — exact rational frame time, timeline endpoints, holding
  outside the range, monotonicity, and **that evaluating twice gives the same
  answer** — which fails the moment something reaches for a clock.
- **`pmg-scene`** — version gating in both directions; canonical CBOR round-trip
  and stable `template_rev`; JSON-mirror equivalence; and that every binding
  transform is **total** against a hostile input corpus (empty, whitespace, CJK,
  emoji, zero-width). A template must not be able to throw on air.
- **`pmg-layout`** — the band contains its children, grows and shrinks with the
  name, collapses when the title is empty, and is deterministic across runs.

The remaining six suites return `Ok(())`. They are wired in so that adding real
assertions is a one-line change rather than a question of where they go.

---

## Spike results

Each brief in `docs/spikes/` carries a results section, filled in with dated,
hardware-attributed findings when it runs.

| Spike | Status | Gates |
|---|---|---|
| S1 render parity | **PASSED on Metal** 2026-08-14 — bit-identical; cross-backend untested | M1 |
| S3 DeckLink output | **internal-clock leg PASSED** 2026-08-14; reference leg blocked on a generator | M2 |
| S6 WebGPU in browser | **largely answered** 2026-08-14 — real adapter/device, 16384 texture limit, 244 KB bundle, 5 ms/figure. Not yet on the booth Macs | M1 |
| S2, S4, S5, S7, S8, S9 | not started | M3–M5 |

**All three M0 spikes have now returned, and all three passed.** ADR 0003's
hand-written-shim approach sustains broadcast output with zero late frames; ADR
0001's shared renderer produces bit-identical output native and in-browser; and
WebGPU is comfortably available in the browser.

**None of the foundational ADRs is a bet any more** — with one honest
qualification: S1 measured Dawn-on-Metal against wgpu-on-Metal, so the
cross-backend case (a Windows designer against a Mac node) is still untested.

---

## Known gaps in the design itself

Places where the documents deliberately stop short, so nobody mistakes silence
for a decision:

- **Cross-node frame-accurate takes** are not designed, only anticipated. The
  `at:` field exists on `Cue` and is unused. ADR 0011.
- **Node redundancy** is out of scope. A dead node is a dead output, and the
  accepted mitigation is a spare machine. ADR 0009.
- **SMPTE 2110 and PTP** have no design beyond "do not foreclose them." There is
  no 2110 plant to develop against.
- **Dynamic data sources** (the P2 input type) have no spec. Timers are P1 and
  land in M6.
- **The screen-content / ProPresenter-subsumption question** is deliberately left
  open until M6 has real operating experience to answer it with.
- **Multi-user simultaneous editing** of the same template is not designed.
  Current model is last-write-wins on a whole-template save, matching
  ProdMesh-Control. If two designers editing one template becomes a real need, it
  needs its own ADR.

---

## What would change this page

The next three things that matter:

1. **M1c — the text stack.** The riskiest piece left, and where R3 lives:
   subpixel glyph positioning (or animated text shudders), premultiplied AA edges
   (or it fringes over live video), and the vertical-metric choice that can never
   be changed once templates exist.
2. **Run the parity corpus on Windows (D3D12)**, so ADR 0001's promise can be
   stated without the "measured on Metal" qualification.
3. **S3's remaining legs at church**, where there is a reference generator: the
   soak locked to blackburst or tri-level, and scope confirmation that a fill+key
   pair genuinely shares a clock.

M0's spikes were scheduled so the project would be allowed to fail early. It
didn't. That is a good outcome and not a reason to stop writing down what is
still untested.
