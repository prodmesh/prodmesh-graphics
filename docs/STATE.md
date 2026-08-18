# State

What is actually real, as opposed to what is designed. Updated as part of the
change that makes something real — not afterwards.

`docs/ROADMAP.md` is where we are going. This is where we are.

**Last updated: 2026-08-17 · M0 and M1 complete · **M2 complete**: layers, playlists and a rundown, adoption, auth, SDI output, preview — and S8 gating it.**

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
| `server/nodeLink.js` | 🟢 handshake, assignment offer/ack, cues with deadlines, replay ring, StateReport adopted. An adopted node that is merely powered off now answers **offline**, not "there is no node by that name" — sessions are live-only, so without a table lookup the message sent an operator to Configure to fix a name that was correct |
| `POST /api/cue` | 🟢 synchronous — 503 `cue_not_acked` when the node stays silent, never a silent success. **Fans across destinations and aggregates the acks** (ADR 0018): a take naming no node goes to every node the Item lives on, and a partial delivery is a 503 carrying `result: "partial"`, a per-destination breakdown, and the sentence — *"On air on part-a. part-offline did not confirm: it is offline."* Deliberately not a 2xx: a client checking `res.ok` would take the button green on 207 |
| Playout mode (ADR 0017) | 🟡 **the model and the operator behaviour are real; the transport is not.** `playlists.mode` (`cue`/`follow`), `PUT /api/playlists/:id/mode`, and the mode travels with the rundown. The keyboard is a function of the focused playlist's mode: in `cue` the arrows move, in `follow` they move and take. Cursor and on-air stay separate marks in BOTH modes — Follow sets nothing on air locally. An unknown mode is refused loudly rather than treated as `cue`. **Not built: the dedicated non-blocking cue socket.** Follow dispatches over `POST /api/cue` without awaiting, so keypresses do not queue behind each other, but the browser→server socket the ADR describes does not exist and Follow has not been exercised at chorus speed against a real node |
| `CueResult::Superseded` (ADR 0017) | 🟢 a take supersedes an unfired take to the same layers, and is **acked** rather than dropped. Batched from what has already arrived — `try_recv` only, so it can never delay a cue waiting to see whether a newer one shows up. Distinct from a late rejection: one is the system working, the other is it refusing, and a three-lamp panel needs different lamps. A replay of a cue that already fired is still `Deduped` |
| Several destinations (ADR 0018) | 🟢 **complete.** One Item, N node/output/layer destinations, each with its own pinned `template_rev` and an `auto`/`on_request` mode. Migration 8 expands every existing Item into a list of one. Assignment fans per node; the node arms all destinations before placing any, so a fan-out is all-or-nothing *within* a node. `auto` fires with the take, `blank` clears its feed as an instruction, and `on_request` waits for **`take_also`** — the "Also full screen" action. 100 server tests |
| `pmg-proto` | 🟢 the wire contract, with checked-in CBOR fixtures the JavaScript tests decode |
| Design / Edit / Show | 🟢 all three real. Design compiles and renders with the shared Rust; Show takes on air and does not lie |
| `pmg-wasm` | 🟢 `compile_template` + `DesignSession` — the browser half of ADR 0001, pinned by a native test |
| Fonts | 🟡 uploaded as CAS assets and fetched by hash. Family matching is still provisional — one face serves every family a template names |
| Auth (ADR 0013) | 🟢 stations, users, groups, permissions, sessions, audit, trusted stations. **An unconfigured server is open and says so; the first user closes it.** 15 tests |
| Adoption | 🟢 an unadopted node is welcomed and given **nothing** — no templates, no items, no cues. Persisted, so it survives a restart |
| mDNS discovery | 🟢 **verified.** A node with no `--server` finds the server and links; two servers are reported rather than guessed between. Needs macOS Local Network permission (see INTEGRATION-NOTES) |
| Configure | 🟢 adopt a node, see its capabilities, edit its layer stack — pushed live |
| S8 chaos harness | 🟢 **gating.** `npm run chaos` — 1000+ takes through five injections, zero fired twice |
| Control API document (M4) | 🟢 `docs/specs/openapi.json` — OpenAPI 3.1 for the surface an integrator writes against, both hosts. **Checked against the routes, both directions**: a documented path that does not exist fails, and a route that is neither documented nor on an explicit not-public list fails too. The second half is the one that earns its keep — a new route forces a decision about whether integrators may call it, rather than being an omission nobody notices. A stale exclusion also fails. JSON not YAML so the test needs no parser and the repo needs no dependency |
| Node API (M4, ADR 0014) | 🟢 **the emergency surface.** `:8520` — `POST /cue` taking the same body shape as the server's endpoint, `GET /status`, and a self-refreshing status page. **Verified with the control server stopped**: a cue applied and returned its frame number, a repeat deduped, a bad layer was rejected by name. A local cue goes on the SAME channel the link uses and is applied by the same loop — same dedupe, same deadline, same superseding — because no adapter gets its own path into the engine. No auth, deliberately: booth LAN, and a credential check against a server that is down is a lock with no key. Ops are a subset (`take`, `take_out`, `clear_layer`, `clear_all`, `arm`); `update` and `timeline` need a keyboard a GPI contact does not have |
| `pmg-node` | 🟢 **the playout binary.** Link client, staging, durable assignment, layer engine, NDI out. Survives server loss; 10 cue-rule tests |
| `pmg-cas` | 🟢 atomic writes, streaming put, verify. Agrees with `server/cas.js` |
| Colour model | 🟢 601/709/2020 RGB→YCbCr with correct narrow spans, and a key path that provably bypasses the matrix. 6 tests |
| Colour config | 🟡 **plumbed, not applied.** Per-output matrix/range/key_range/key_mode reach the node; the backends still let the card convert. See INTEGRATION-NOTES |
| Test patterns | 🟢 on demand, above the layer stack, so a configuration can be proved without clearing the show off air |
| Media ingest | 🟢 probe, judge, transcode to all-intra. Long-GOP converted, ProRes passed through with alpha intact, non-media refused. Jobs, one at a time. 6 tests |
| `pmg-media` | 🟢 **stills and video.** PNG/JPEG in process; clips via a child ffmpeg over a pipe, premultiplied at the boundary, VFR refused. Exact-rational A/V clock. 9 tests |
| Images on air | 🟢 `kind: "image"` with `fit: cover \| contain \| stretch`, decoded once at assignment, soft alpha composites correctly over a key |
| M3 two-hour soak | 🟢 **passed.** 894 cues all applied, **zero late frames over 432,000**, audio drift +2.3 ms with the whole excursion inside one frame, zero warnings. Recovery from a stall proved separately by injecting one |
| Live rebind (M3g) | 🟢 **the differentiator, working on SDI.** `Update` morphs between two solved layouts — the box grows and shrinks to fit new text while on air. Rebind solves in 0.2–0.7 ms, zero frames over budget, verified on the card by eye. `clip` is now implemented, which it had not been since M1d |
| Video on air | 🟢 `kind: "video"` with `fit` and `loop`. **21 s of keyed 1080p ProRes 4444 at 59.94: 0 held, 0 frames over budget**, worst 15.17 ms of 16.68. The upload costs ~2.7 ms of headroom. A clip starts up to 120 ms after the take, holding its first frame — pre-arm is the fix and needs the preview slot |
| Video audio | 🟢 **frame-synced and self-correcting.** A 2 h soak found a hiccup left audio +168 ms ahead permanently; the card's buffer level now drives the sample count. Verified by INJECTING a 1 s stall: settles to **−0.6 ms of target**, where it used to stay 164.8 ms out. Otherwise: Exact 801/801/801/801/800 cadence at 59.94; two hours drifts zero samples. **SDI verified on a Quad 2** — flat card buffer over 2 min, and a deliberately naive cadence drains it, so the instrument is proven. **NDI verified by send-and-probe** — 801 samples, level unchanged at −12.0 dBFS |
| `pmg-audio` | 🟢 the audio clock and layer→pair mixing. Sums and clips rather than averaging. 10 tests, two-hour non-drift in `--selftest` |
| Genlock status | 🟡 read live from the card and surfaced in Configure; what `present: true` means with nothing plugged in is **unverified** — needs a reference generator |
| Multi-output | 🟢 **1–N outputs per node**, a render thread per output GROUP (shared clock), independent content and resolutions per output. Two outputs at 59.94, zero late |
| `pmg-preview` | 🟢 **H.264 over a WebSocket, decoded with WebCodecs.** Verified in a real browser: 103 frames on a canvas, joiner paints in 4 ms from the cached keyframe, 0.05 Mbit/s on static content. MJPEG kept behind `--preview-mjpeg` as the no-decoder fallback |
| `pmg-out-decklink` | 🟢 **SDI output.** Fill and key on two ordinary ports, 90 s at 1080p59.94, zero late frames. The key *picture* is unverified on a scope |
| `pmg-out-ndi` | 🟢 the shared sender — one unpremultiply loop for the node and the CLI |
| `streamHub` | ✅ ported from ProdMesh-Control — refcounted topics, conflation, snapshot-on-join |
| `cueLog` | ✅ the log-tail bridge from ADR 0008, with gap recovery. 6 tests |
| PMG Link | ⬜ spec only |
| CAS | ⬜ spec only |
| Web app | 🟡 four-mode shell, routing, tokens, `useTopic` + `useQuery`. 6 tests. No functionality |
| `pmg-scene` | 🟢 **full document model**, canonical CBOR + JSON mirror, `template_rev`, validation, manifest, total binding evaluation. 25 tests |
| `pmg-layout` | 🟢 **flexbox solver** over `TextMeasure` (taffy, ADR 0016). Shrink-and-grow and `visible_if` removal verified as numbers. 12 tests |
| `pmg-text` | 🟢 fonts **from bytes**, swash shaping, subpixel glyph atlas at ⅛ px, real `TextMeasure`, **and line breaking** — hard `\n` plus greedy wrapping to the solved box, lines cut out of one shaped run so kerning does not shift when a later word wraps. Whitespace only: not UAX #14, so no hyphenation or CJK — that still wants parley. 20 tests. R3's three hazards structurally answered; quality still needs S2 |
| Colour pipeline | 🟢 **composites in linear, encodes once at output** (ADR 0012, which said so since M0 and was not implemented until 2026-08-17). Scene draws into `Rgba16Float`; one full-screen pass encodes to `Rgba8Unorm`. Un-premultiplies before the transfer function and re-premultiplies after, because `encode(a·c) ≠ a·encode(c)` and the error is largest at the low alphas a soft edge is made of. Alpha is never converted — it is coverage, and the key must stay a monotone alpha signal. Encoding is per FRAME, not per layer |
| Coverage gamma | 🟢 **stem darkening in the rasterizer**, γ = 1/1.45. Geometric coverage is right when composited in linear light and too dark when a downstream keyer blends the encoded values — a 50%-covered edge of white type lands at 128/255 and displays as 21% of the light it should carry. We cannot fix the switcher, so the compensation goes where the coverage is made, as FreeType and Skia both do. Approximate by nature: the exact correction depends on a background this process never sees. Measured lift 138.5 → 151.0 mean edge alpha with solid pixels and edge count unchanged |
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
