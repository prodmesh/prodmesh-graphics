# Testing

Five layers, and an honest line between what CI can certify and what only a real
Sunday can.

| Layer | Tool | Runs |
|---|---|---|
| 1. Rust unit and property tests | `cargo test` | every push |
| 2. Golden images | `pmgctl render-golden` + `parity.yml` | every push |
| 3. `pmgctl --selftest` | one binary, prints `PASS` | every push, **gates releases** |
| 4. Chaos harness | `pmgctl chaos` | every push from M2 |
| 5. Manual on-air checklists | a human, real hardware | before each milestone ships |

## 1 — Rust unit and property tests

Normal `cargo test`. Three property tests carry unusual weight and must not be
deleted or weakened:

- **CBOR ⟷ JSON round-trip.** A SceneDoc encoded to canonical CBOR, decoded,
  re-emitted as the JSON mirror, and parsed back must be identical. The
  `template_rev` hash depends on canonical encoding being deterministic; if this
  fails, revisions are not stable and nothing above it is trustworthy.
- **Unknown-field preservation.** A doc containing keys and node kinds the reader
  does not understand must survive a read/write cycle byte-identically in those
  regions. This is the forward-compatibility guarantee from ADR 0005.
- **Timeline evaluation is pure.** Evaluating a timeline at time *t* depends only
  on the timeline and *t*. No wall clock, no ambient state.

## 2 — Golden images

`testdata/templates/*.pmgscene.json` compile and render to
`testdata/golden/*.png`. Two things are checked, and they are different failures:

- **Native regression** — did a change to layout, text, or the renderer move
  something that was not supposed to move?
- **Native ⟷ WASM parity** (`parity.yml`, spike S1) — does the browser produce
  the same picture as the node?

**The parity bar:** ≥99.9% of pixels within 1/255, max channel delta ≤2/255, and
**zero geometric displacement**. Those are reported separately on purpose. A
glyph two pixels to the left is a bug in the design promise; a shade one step off
is a GPU backend difference we have already accepted. Reporting them as one
number hides the one that matters.

**A wgpu version bump is a release-gated change requiring a full golden re-run.**
Not a routine dependency update.

**Updating a golden is a deliberate act.** The commit that changes a golden image
says in its message what visually changed and why it is correct.

## 3 — `pmgctl --selftest`

One binary, no test framework, prints `PASS` or a specific failure. Inherited
directly from prodmesh-caption and prodmesh-rta, and for the same reasons: it
runs on the machine that has the problem, it needs no toolchain beyond the
shipped binary, and it gates releases in CI.

Covers: scene round-trip and hashing, layout against goldens, timeline
evaluation, colour conversion including the key-channel path, CAS put/get/verify,
and PMG Link message encode/decode.

**Always run it after touching `pmg-scene`, `pmg-layout`, `pmg-anim`, or
`pmg-color`.**

```bash
cargo run -p pmg-cli -- --selftest    # must print PASS
```

## 4 — Chaos harness (from M2)

`pmgctl chaos` exists because the cue path's guarantees are only real if
something actively tries to break them. It fires 1000 takes while:

- killing the WebSocket mid-cue,
- blackholing the link for 30 seconds,
- restarting the server,
- restarting the node,
- and issuing duplicate `cue_id`s, as an operator's double-click would.

**Two assertions, both absolute:**

1. Final on-air state matches intent exactly.
2. **No take fires twice.**

This becomes a gating selftest, like the C++ siblings' `--selftest`. If it is
flaky, that is a finding about the protocol, not about the test.

## 5 — Manual on-air checklists

What CI structurally cannot certify. Run before a milestone ships.

### Before any milestone

- [ ] `pmgctl --selftest` prints `PASS` on the actual playout machine, not just CI.
- [ ] Every node in Configure shows green, with its reference state correct.
- [ ] Every output shows its test pattern and it looks right on the switcher's
      multiviewer — not just on a confidence monitor.
- [ ] Ramp test pattern: 0% and 100% markers land where the spec says, at the
      configured range.

### M1 — the vertical slice

- [ ] Design a lower third; the text box grows and shrinks with the name.
- [ ] `visible_if` collapses the second line when the title is empty.
- [ ] Publish; the node reports ready.
- [ ] Take. The In animation is smooth — **watch the text edges specifically for
      shudder** (R3).
- [ ] Take out. The Out animation completes and the layer clears.
- [ ] The designer's preview and the NDI output look the same. Photograph both.

### M2 — the operator

- [ ] Take a lower third while a bug is up. The bug does not move.
- [ ] Clear layer removes one; clear all removes both.
- [ ] Pull the network cable mid-service. **What is on air stays on air.**
- [ ] Plug it back in. The server adopts the node's state without re-firing.
- [ ] Restart the control server mid-service. **Nobody in the room notices.**
- [ ] Double-click take. It fires once.

### M3 — hardware

- [ ] Fill and key into the switcher's keyer. Soft edges are clean over both a
      bright and a dark background — the premultiplication test (R3, R9).
- [ ] Video with alpha plays; audio is in sync; lip-sync pattern confirms.
- [ ] Pull reference. Configure says so within seconds.
- [ ] Two hours of video looping on one layer with lower thirds firing on
      another: zero dropped frames.

### M4 — control surfaces

- [ ] **Stop the control server. Every surface still fires a take.** This one
      checklist item is the proof of ADR 0009's invariant, and it is the reason
      GPI, OSC, and MIDI terminate at the node.

## What the tests deliberately do not cover

- **Drag gestures in the Design canvas.** jsdom has no layout, so a simulated
  drag certifies nothing. The canvas is built so every operation is reachable
  without a drag — buttons and keyboard — and those paths are tested. The gesture
  itself is hand-verified. (Same reasoning as ProdMesh-Control's layout editor.)
- **Real GPU output in CI.** CI runs headless with a software adapter. Anything
  about actual hardware timing is a spike result or a manual checklist item, and
  it lands in `docs/INTEGRATION-NOTES.md` dated and hardware-attributed.
- **Long-running thermal behaviour.** Only a soak on the real machine finds that.
