# Roadmap

The uber-plan. Seven milestones, each a shippable increment with a demonstrable
outcome and a verification you can actually run. What is *deferred* at each stage
is listed as deliberately as what ships — the deferrals are the plan.

`docs/STATE.md` tracks where we actually are. This document is where we are
going.

| Milestone | Outcome | Gating spikes |
|---|---|---|
| **M0** | Scaffold, docs, CI. Learn whether the foundational bets hold. | S1, S3, S6 |
| **M1** | The lower-third vertical slice, on NDI. | — |
| **M2** | Layers, playlists, the operator, DeckLink. | S8 |
| **M3** | Media, multi-output, genlock, colour. | S4, S5, S7, S9 |
| **M4** | Control surfaces. | — |
| **M5** | Authoring maturity. | S2 |
| **M6** | Productization seams. | — |

---

## M0 — Skeleton and truth-finding

**This milestone's purpose is to be allowed to fail.** Everything after it
assumes three things are true that nobody has verified on this hardware: that a
Rust renderer produces the same picture natively and in a browser, that DeckLink
can be driven cleanly from Rust without dropping frames, and that WebGPU is
usable in the booth's actual browsers. If S1 or S3 goes badly, ADRs 0001, 0002,
and 0003 get revisited *before* M1 rather than after.

**Ships**
- Repo scaffold: Cargo workspace where all 17 crates build, `sys/` shims with
  their hand-written headers, npm project with the Express server and the React
  four-mode shell, `testdata/` layout.
- `pmgctl --selftest` printing `PASS`.
- CI: `build.yml` (cargo + the `wasm32-unknown-unknown` pure-set job + node tests
  + the selftest gate), `parity.yml`, `release.yml` with tag-version verification.
- The full document set — this file, the ADRs, the specs, the spike briefs.
- Rust toolchain pinned and installed.

**Spikes** — S1 render parity, S3 DeckLink output soak, S6 WebGPU in the browser.

**Demonstrable:** selftest `PASS` in CI; a moving 1-pixel line on SDI for ten
minutes with zero late frames; a triangle and a text block rendered identically
in Chrome and natively, with the diff numbers written down.

**Deferred:** everything else. No engine code beyond what a spike needs.

---

## M1 — The lower-third vertical slice

Design a responsive lower third in the browser → publish → push to a node → type
a name and title in Edit → take it on and off air with In and Out animation, over
**NDI with key and fill**.

This is the slice that proves the architecture, because it touches every layer
once: scene format, layout, text, renderer, WASM designer, control server, PMG
Link, CAS, and output.

**Why NDI and not SDI.** M1 already requires the entire stack above to work for
the first time. Adding hardware frame-timing risk on top of that is what turns a
slice into a quarter. NDI is pure software, carries alpha natively, and the demo
an operator sees is identical. The DeckLink spike still runs in M0, so we learn
early; the DeckLink *backend* lands in M2 on a renderer that already works.

**Ships**
- SceneDoc v1: `box`, `text`, `shape`. Canonical CBOR, JSON mirror, round-trip
  property test.
- Flexbox layout with content sizing — the shrink-and-grow behaviour.
- Text binding with the fixed transform set, and `visible_if`.
- `In` and `Out` timelines, cubic easing, **transform-only animation**.
- One node, one output, one layer, one Item.
- Control server: templates, items, nodes REST; PMG Link; CAS with asset sync.
- Design mode: a crude but real canvas — layer tree, properties panel, WASM
  preview.
- Show mode: a take button that tells the truth.

**Verification**
- `pmgctl --selftest` covers scene round-trip, layout goldens, timeline
  evaluation.
- `npm run test:slice` publishes a fixture template, boots a node with
  `--output null`, fires a take, and asserts rendered frames at t=0/400/800 ms
  against goldens.
- The manual on-air checklist in `docs/TESTING.md`, run against a real NDI
  receiver.

**Deferred:** video, images, multiple layers, playlists, preview stream, auth,
discovery (type the IP), multiple outputs, genlock, DeckLink.

---

## M2 — Layers, playlists, and the operator

The milestone that makes it usable by someone who is not the person who built it.

**Ships**
- Named layer stacks per output; `clear-layer` and `clear-all`.
- Item destinations; Playlists; Rundown ordering.
- The full cue protocol: arm / take / take-out / update, `seq` and acks, the
  replay ring, deadline rejection, resync after disconnect, `StateReport`
  adoption.
- mDNS discovery and node adoption in Configure — with manual IP entry always
  available.
- The ADR 0013 auth model: stations, users, groups, permissions, audit.
- DeckLink output backend, including fill+key on two ports of one device.
- MJPEG preview — deliberately the cheap version, to get pixels on screen.

**Verification**
- The chaos harness (S8) becomes a gating selftest: kill the link mid-cue,
  blackhole it for 30 s, restart the server, restart the node, all while firing
  1000 takes. Final on-air state must match intent exactly and **no take may
  fire twice**.
- A 20-item rundown across 3 layers running unattended for an hour with induced
  disconnects, ending in the expected state.

**Deferred:** video and audio, WebCodecs preview, multiple outputs, genlock,
GPI/MIDI/OSC, RossTalk, Companion.

---

## M3 — Media, multi-output, and colour

Where it stops being a graphics toy and starts being broadcast equipment.

**Ships**
- Images; video with alpha; frame-synced embedded audio on SDI channel pairs.
- Server-side ingest transcode to an all-intra intermediate, with a clear
  message when a source is rejected.
- 1–4 phase-locked outputs per node; output groups and their render threads.
- Genlock/reference configuration and lock status surfaced in Configure.
- Per-output fill / key / fill+key / embedded alpha.
- Colour space, matrix, and range configuration, with the test patterns that
  prove them.
- WebCodecs-over-WebSocket preview replacing MJPEG.

**Verification**
- S9's colour suite run against real hardware, with results recorded — dated and
  hardware-attributed — in `docs/INTEGRATION-NOTES.md`.
- A two-hour soak with video looping on one layer and lower thirds firing on
  another: zero dropped output frames, measured A/V drift within a frame.

**Deferred:** PTP, cross-node take synchronisation, dynamic data sources, timers.

---

## M3g — The responsive tween ✅

**Done 2026-08-17, verified on SDI.** The rebind solves in 0.2–0.7 ms, no frame
goes over budget with a morph running, and the box grows and shrinks to fit —
watched on a DeckLink at 1080p59.94. Implementing it also implemented `clip`,
which the scene format had declared and the renderer had ignored since M1d.

Two bugs reached the card that the frame harness structurally could not find —
a renderer cache keyed on a name rather than on the instance, and an outgoing
pass clipped by the rect it started at. Both are in INTEGRATION-NOTES.

**Added 2026-08-16, and it jumped the queue.** The Canyon Hills rundown
(`docs/reference/`) marks exactly one thing in sixty minutes as impossible on the
current system: a lower third whose box **grows and shrinks to fit new text while
it is on air**, cross-fading between two states. That is what this project's
flexbox solver was built for, and today the node returns
`RejectReason::Unsupported` for `CueOp::Update`.

The refusal is honest and the fix is already shaped by the architecture: layout
is solved at arm in 0.3–2 ms, so **solve both states at arm and interpolate
between two solved rect sets**. Not a relayout per frame — the arm/take split
still forbids that, for measured reasons — but two layouts and a tween, which is
why it is affordable.

**Ships:** an `Update` cue that rebinds live, both layouts solved at arm, rect
interpolation with an easing, glyph cross-fade. **Verification:** a Bible verse
moving to its next section on air, with the box tweening, watched on SDI.

**~~Also missing and not yet placed: an audio-only output backend.~~** Landed
early, as M6m: an output's audio may leave on a host sound device — CoreAudio,
WASAPI, ALSA — instead of being embedded in SDI or NDI. See ADR 0028. What is
still missing is an output with **no video at all**, which is a routing
question rather than a transport one and needs an answer to "which layers'
audio".

---

## M4 — Control surfaces

**Blocked on two model changes, deliberately.** ADR 0017 (playout mode per
playlist) and ADR 0018 (one Item, several destinations) both alter what a cue
means: a take can now be superseded, and it can now be partly delivered. Every
surface in this milestone is an adapter over that model, so building five of them
first means rewriting five of them after. **Land 0017 and 0018, then M4.**

**The rule that shapes this milestone: the open WS/REST API is the only real
surface, and every other one is a thin adapter over it.** Building five surfaces
in parallel means five half-tested paths into a live show. One path, tested hard,
with adapters on top.

**Ships**
- The documented, OpenAPI'd control API.
- A Bitfocus Companion module → `~/Code/bitfocus-companion-modules`.
- A RossTalk listener.
- A DashBoard `.grid` panel → `~/Code/ross-dashboard-panels` (use the
  `ross-dashboard-grid` skill).
- GPI, OSC, and MIDI — **terminating at the node**, not at the server, because
  that is what makes them work when the server is down.
- LTC / MIDI timecode as a **clock the rundown follows**, which is a different
  thing from a surface that presses a button, and is how advanced services run
  the worship set.
- Surfaces report `Superseded` distinctly from `Rejected` (ADR 0017), and a
  partly-delivered take distinctly from a failed one (ADR 0018). A panel with
  three lamps needs those to be different lamps.
- ProdMesh-Control as a consumer of the API.

**Verification:** every surface fires a take **with the control server stopped**.
That single checklist item is the proof of the ADR 0009 invariant. Plus API
contract tests.

**Deferred:** anything that would put a surface in the program path.

---

## Open modelling questions — raised 2026-08-15, not yet decided

Two requirements from the owner that the current data model cannot express.
Both need an ADR **before** they are built, because they change the schema
rather than the screen. Captured in `docs/UX-BRIEF.md` §4–5.

**Playout mode per playlist.** The original brief's "playout controls that vary
by playlist", made concrete: a **Follow** mode where navigating *is* taking
(lyrics, arrow keys, no arm step) and a **Cue** mode of explicit arm-then-take
(the message, media-heavy, an Xpression-style cuelist). The cursor's meaning and
the keyboard map both change with the mode. The primitives exist — cursor and
take are already separate operations — but a Playlist has no `mode` and nothing
consumes one.

**One Item, several destinations.** A lyric slide goes to a DeckLink fill+key
pair *and* to an in-room LED wall over NDI, with a different treatment and
possibly a different template for each. An Item today has exactly one
`destination: { output, layer }`. This is the larger of the two gaps.

---

## M5 — Authoring maturity

The milestone that makes Design something a volunteer designer can use, rather
than something the author of the format can use.

**Ships**
- Arbitrary named timelines beyond In/Out — Update, Highlight, Loop.
- A keyframe editor with curves.
- Masks, gradients, shadows, text effects.
- Template variants and themes.
- Sample-data preview; an asset browser; undo/redo.
- Template revision UI, including "update on-air items" with a real diff.
- The migration machinery exercised for real on a v1.0 → v1.1 step.

**Verification:** the golden corpus grows past 50 templates, and S1's parity
suite runs across all of them on every CI run. **Done 2026-08-20** — 89
templates plus the 3 primitives, native and browser bit-identical across all 92;
`parity.yml` renders and diffs the whole corpus per push and fails if the count
drops below 50 or a text figure was skipped.

**Still open in this milestone**, and deliberately named rather than quietly
dropped — one of three, the other two closed on 2026-08-20:

- **Template variants.** "Variants and themes" shipped its themes half (ADR
  0021, format 1.2, the manager and the apply sweep). Variants — a template
  family with field compatibility enforced at publish — are not started.
- ~~**Masks.**~~ **Done 2026-08-20** (format 1.3, §3.5) — a mask node masks its
  siblings through a rounded-box shape with its own transform, which is what a
  wipe reveal is. Still deliberately out: an inverted mask, a feathered edge,
  and masking by an arbitrary alpha (a glyph shape, an image's luma).
- ~~**Curve editing.**~~ **Done 2026-08-20** — two draggable control points in
  CSS's convention, overshoot allowed.

---

## M6 — Productization seams

**Ships**
- ~~Multi-screen destinations~~ **done 2026-08-21.** ADR 0018 gave an Item N
  destinations; ADR 0023 gave those destinations a **screen** — a named place,
  configured once, that an Item targets instead of a raw node/output/layer
  triple. Moving a wall to another node is one edit in Configure and rewrites
  no Items. The screen supplies the default treatment and the Item still pins
  the revision, so a publish is not a live change to everything on that screen.
- Timers and utility data sources — the P1 input types.
- Export and import bundles, with the `include_fonts` licensing flag.
- Licensing hooks.
- An installer, and documentation for someone outside our building. (A node's
  own hardware is configurable from Configure as of ADR 0024, which is the half
  of "installable" that is about the machine rather than the package.)
- PTPv2 investigation.
- The honest evaluation of whether screen content should move here.

**Deferred until proven necessary, deliberately:** node redundancy, SMPTE 2110,
HDR. Each is a real project; none is a v1 problem, and designing for all three
speculatively would cost more than adding any one of them later.

---

## Spikes and what they gate

Each has a brief in `docs/spikes/` with its question, method, success bar, and a
results section to fill in — dated and hardware-attributed, per family
convention.

| Spike | Question | Gates |
|---|---|---|
| **S1** | Do native wgpu and browser WebGPU produce the same picture? | M0 → M1 |
| **S2** | Is Rust text good enough for broadcast, especially animated? | M5 |
| **S3** | Can Rust drive DeckLink output cleanly for ten minutes? | M0 → M2 |
| **S4** | What does reference actually phase-lock, across machines? | M3 |
| **S5** | Can we seek and play frame-accurately with synced audio? | M3 |
| **S6** | Is WebGPU usable in the booth's browsers? | M0 → M1 |
| **S7** | What is glass-to-glass preview latency, and at what cost? | M3 |
| **S8** | Does the cue path survive chaos without losing or duplicating? | M2 |
| **S9** | Is the colour and key output numerically correct? | M3 |
