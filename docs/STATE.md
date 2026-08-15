# State

What is actually real, as opposed to what is designed. Updated as part of the
change that makes something real — not afterwards.

`docs/ROADMAP.md` is where we are going. This is where we are.

**Last updated: 2026-08-14 · milestone M0 · nothing has run on hardware yet.**

---

## The honest summary

There is no engine. There is a repository with a complete design written down, a
Cargo workspace whose crates compile as empty stubs, and a web app that serves an
empty four-mode shell. The three M0 spikes that would tell us whether the
foundational bets hold have not been run.

**Nothing in this repository has rendered a pixel or touched a piece of video
hardware.**

---

## Component status

| Component | State |
|---|---|
| Documentation set | ✅ complete — 15 ADRs, 7 specs, 9 spike briefs |
| Cargo workspace | 🟡 stubs — all 19 crates compile; `fmt`, `clippy -D warnings`, and the wasm32 pure-set job are green |
| `pmgctl --selftest` | 🟡 prints `PASS` with **real** assertions in three crates (below); the rest are placeholders |
| `sys/decklink-sys` | 🟡 header written, shim not implemented |
| `sys/ndi-sys` | 🟡 header written, shim not implemented |
| Control server | 🟡 boots; health, version, and the topic stream. 10 tests |
| SQLite schema | ⬜ sketched in `server/db.js`, not created |
| `streamHub` | ✅ ported from ProdMesh-Control — refcounted topics, conflation, snapshot-on-join |
| `cueLog` | ✅ the log-tail bridge from ADR 0008, with gap recovery. 6 tests |
| PMG Link | ⬜ spec only |
| CAS | ⬜ spec only |
| Web app | 🟡 four-mode shell, routing, tokens, `useTopic` + `useQuery`. 6 tests. No functionality |
| Design canvas | ⬜ not started |
| Renderer | ⬜ not started |
| Text stack | ⬜ not started |
| Layout | ⬜ types only, no solver |
| Any output backend | ⬜ not started |
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

None yet. Each brief in `docs/spikes/` has an empty results section that gets
filled in with dated, hardware-attributed findings when it runs.

| Spike | Status | Gates |
|---|---|---|
| S1 render parity | not started | M1 |
| S3 DeckLink output | not started | M2 |
| S6 WebGPU in browser | not started | M1 |
| S2, S4, S5, S7, S8, S9 | not started | M3–M5 |

**S1 and S3 are the ones that could invalidate ADRs 0001, 0002, and 0003.** Until
they run, treat those three decisions as well-reasoned bets, not settled facts.

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

1. **S1 run**, with numbers written into `docs/spikes/S1-render-parity.md` and
   `docs/INTEGRATION-NOTES.md`. Needs `pmg-render` to draw a triangle and a text
   block — which is the smallest amount of renderer that can answer the
   question, and deliberately so.
2. **S3 run** on real DeckLink hardware, with the ten-minute zero-late-frame
   result confirmed or refuted.
3. **S6 run** on the actual booth Macs, not a dev laptop.

Until then ADRs 0001, 0002, and 0003 are well-reasoned bets, and this page says
so on purpose.
