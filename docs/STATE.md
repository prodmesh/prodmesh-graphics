# State

What is actually real, as opposed to what is designed. Updated as part of the
change that makes something real — not afterwards.

`docs/ROADMAP.md` is where we are going. This is where we are.

**Last updated: 2026-08-14 · milestone M0 · S1, S3, and most of S6 passed.**

---

## The honest summary

There is no engine, and nothing has rendered a scene. There is a repository with
a complete design written down, a Cargo workspace of mostly-stub crates, and a
web app serving an empty four-mode shell.

**But both vendor boundaries are real, measured, and one of them is proven.** NDI
sends and receives frames through a hand-written, offset-verified binding.
DeckLink enumerates, probes, and — as of today — **sustains 1080p59.94 for ten
minutes with zero late and zero dropped frames**, which validates ADR 0003's
hand-written-shim approach for output.

`docs/INTEGRATION-NOTES.md` is no longer empty; six entries came out of one
afternoon, and three of them were version-skew failures that each presented as
something other than what they were.

**S1 (render parity) and S6 (WebGPU in the booth) have still not run**, so ADRs
0001 and 0002 remain bets.

---

## Component status

| Component | State |
|---|---|
| Documentation set | ✅ complete — 15 ADRs, 7 specs, 9 spike briefs |
| Cargo workspace | 🟡 stubs — all 19 crates compile; `fmt`, `clippy -D warnings`, and the wasm32 pure-set job are green |
| `pmgctl --selftest` | 🟡 prints `PASS` with **real** assertions in three crates (below); the rest are placeholders |
| `sys/decklink-sys` | 🟢 enumeration, probing, modes, reference, **and scheduled output — 10 min at 1080p59.94, zero late, zero dropped** |
| `sys/ndi-sys` | 🟢 runtime loading, find, receive, send — **frames verified in both directions**. ABI asserted statically and at runtime |
| `pmg-color::patterns` | 🟢 ramp, bars, soft-edge, alpha staircase, moving line — the diagnostic instruments from `output-config.md` §8 |
| `pmgctl ndi` / `pmgctl decklink` | 🟢 info, probe, send / list, modes, ref |
| Control server | 🟡 boots; health, version, and the topic stream. 10 tests |
| SQLite schema | ⬜ sketched in `server/db.js`, not created |
| `streamHub` | ✅ ported from ProdMesh-Control — refcounted topics, conflation, snapshot-on-join |
| `cueLog` | ✅ the log-tail bridge from ADR 0008, with gap recovery. 6 tests |
| PMG Link | ⬜ spec only |
| CAS | ⬜ spec only |
| Web app | 🟡 four-mode shell, routing, tokens, `useTopic` + `useQuery`. 6 tests. No functionality |
| `pmg-wasm` | 🟡 the parity harness — renders the corpus in-browser and posts raw RGBA back. 244 KB |
| Design canvas | ⬜ not started |
| `pmg-render` | 🟡 the S1 parity corpus only — 3 figures on wgpu 30, native **and** wasm32. No scene rendering |
| Text stack | ⬜ not started |
| Layout | ⬜ types only, no solver |
| `pmg-out-ndi` / `pmg-out-decklink` | ⬜ not started — `pmgctl` drives `sys/` directly for now |
| CI | 🟡 workflows written and green locally; **never run on GitHub Actions** — no remote yet |
| Rust toolchain on the dev box | ✅ 1.90.0 + `wasm32-unknown-unknown` |

### What `--selftest` actually asserts today

Three crates carry real assertions, chosen because they are small, fully
specified, and guard the rules most likely to be broken by someone doing the
obvious thing:

- **`pmg-color`** — narrow/full range endpoints, and **key chroma is neutral at
  every alpha in every range**. That is the assertion that catches someone
  reusing the fill path for the key (ADR 0012 §3).
- **`pmg-anim`** — exact rational frame time, timeline endpoints, holding
  outside the range, monotonicity, and **that evaluating twice gives the same
  answer** — which fails the moment something reaches for a clock.
- **`pmg-scene`** — version gating in both directions, and that every binding
  transform is **total** against a hostile input corpus (empty, whitespace, CJK,
  emoji, zero-width). A template must not be able to throw on air.

The other seven suites return `Ok(())`. They are wired in so that adding real
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

1. **Start M1.** The foundational bets have all returned green, so the scene
   format, layout solver, and text stack are now the critical path rather than
   the risk.
2. **Run the parity corpus on Windows (D3D12)**, so ADR 0001's promise can be
   stated without the "measured on Metal" qualification.
3. **S3's remaining legs at church**, where there is a reference generator: the
   soak locked to blackburst or tri-level, and scope confirmation that a fill+key
   pair genuinely shares a clock.

M0's spikes were scheduled so the project would be allowed to fail early. It
didn't. That is a good outcome and not a reason to stop writing down what is
still untested.
