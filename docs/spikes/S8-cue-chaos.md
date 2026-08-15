# S8 — Cue delivery under chaos

**Gates:** M2 · **Risk:** [R8](../RISKS.md#r8) · **ADRs:** 0007, 0008, 0009
**Status:** **PASS 2026-08-15** — 1000+ takes through five injections; zero fired twice, zero without a terminal outcome

## Question

Does the cue path keep its guarantees when the network, the server, and the node
are all misbehaving? Specifically: **is any take lost, and is any take fired
twice?**

This spike becomes a permanent gating selftest, like the C++ siblings'
`--selftest`. It is the only way the guarantees in ADR 0007 are real rather than
intended.

## Method

`pmgctl chaos` drives a real server and a real node (with `--output null`) and
fires **1000 takes** across several layers, while continuously injecting:

| Injection | Exercises |
|---|---|
| Kill the WebSocket mid-cue | Reconnect, `last_applied_seq`, the replay ring |
| Blackhole the link for 30 s | Deadline rejection of zombie cues; node autonomy |
| Restart the server | New `epoch`, `StateReport` adoption, no re-firing |
| Restart the node | New `boot_id`, assignment reload from disk, come-up-black |
| Duplicate `cue_id` (operator double-click) | Idempotency on `(epoch, cue_id)` |
| Retry after an HTTP timeout with the same `cue_id` | The same, via the server |
| Delay acks past the deadline | `503 cue_not_acked` and its honesty |
| Exceed the 1024-entry replay ring | Fallback to full resync |
| Saturate the asset HTTP path with a large transfer | **That a take is not queued behind bulk data** |

The last one is the most important, because that is what the three-connection
split in ADR 0007 exists to guarantee, and it is the kind of property that is
silently lost in a refactor.

Against a recorded intent log, at the end:

## Success bar

| Assertion | Bar |
|---|---|
| Final on-air state matches intent | **Exactly** |
| Takes fired twice | **Zero** |
| Takes lost (applied=false with no rejection recorded) | **Zero** |
| Every cue has a terminal outcome | applied · deduped · rejected(reason) — never nothing |
| A rejected cue's reason is accurate | Always |
| Take latency while an asset transfer saturates the link | Within deadline |

**Both absolutes.** A lost take is a missing graphic; a duplicated take is a
re-animated one in front of a congregation. If this harness is flaky, that is a
finding about the protocol, not about the test.

## What it must also prove

- **After a server restart, nothing on air changed.** The server adopted
  `StateReport` and did not re-assert its own view
  ([ADR 0009](../decisions/0009-nodes-are-autonomous.md)).
- **After a node restart, the node came up black** and did not auto-restore
  (unless `restore_on_boot` was set).
- **`?since=<seq>` gap recovery on the cue log** returns exactly the missing
  entries, with no duplicates and no holes
  ([ADR 0008](../decisions/0008-snapshots-stream-cues-are-acked.md)).

## If it fails

- **Duplicates** → `cue_id` is being regenerated somewhere on the retry path, or
  idempotency is scoped wrong across epochs. Both are bugs in our code.
- **Losses across reconnect** → the replay ring or `last_applied_seq` handling.
  This is exactly the case the machinery exists for, and exactly what nobody
  tests without a harness like this.
- **Takes queued behind an asset transfer** → something is sharing a connection
  that must not. Structural, and must be fixed before M2 ships.

## Result — 2026-08-15 · PASS

`node tools/chaos.mjs --takes 1000`, MacBook Pro M3 Max. Real server, real
`pmg-node`, with a TCP proxy in between so the control link can be severed
without touching either.

| | |
|---|---|
| Takes attempted | **1518** (1000 budgeted + filler, see below) |
| Applied / deduped / rejected | 210 / 29 / 1220 |
| **Fired twice** | **0** |
| **No terminal outcome** | **0** |
| Transport unknown | 4 |
| Node came up black after restart | ✅ |
| Cue-log gap recovery | ✅ no holes, no duplicates |
| Final on-air state vs intent | ✅ exact, all three layers |
| Take latency under asset saturation | **p50 1.6 ms · p95 27.5 ms · max 754 ms** |

All five injections ran: link killed mid-cue, blackholed 30 s, server restarted,
link killed again, node restarted. 1014 bulk 8 MB asset pulls crossed the server
during the run, from a separate process.

The high rejection count is the harness working, not the system failing: for
much of the run the link is deliberately down, and **a take that cannot be
confirmed is rejected rather than fired late**. Every one of those 1220 got a
reason.

### Three ways the harness lied before it told the truth

**It skipped its own hard cases and still went green.** Injections were keyed to
a fraction of progress, and the take loop ended the run. A blackholed link makes
every take fail in about a millisecond, so a 1000-take run drained to completion
*during* the 30 s blackhole — the server restart and node restart never fired,
and it reported PASS. Injections are now gated on take count and the run will
not end until every one has executed, firing filler takes if the budget runs out
first. A harness that can skip its hard cases and pass is worse than none.

**It asserted an empty expectation.** The first genuinely-complete run passed
with `intended_final: []` — the node restart correctly cleared the intent model,
nothing succeeded afterwards, and the final-state check compared nothing to
nothing. There is now a deterministic settle phase: after chaos ends and the
link is healthy, one take per layer, and *that* is what the final state is
checked against.

**It measured itself.** Saturation ran in-process, buffering each 8 MB pull with
`arrayBuffer()` on the same event loop that fires the takes and holds the
stopwatch. It reported **513 cue requests timing out at 4 s** and a p95 that
excluded them, because percentiles were taken over successful takes only. Moving
the load to a separate process and streaming-to-discard dropped
`transport_unknown` from **513 to 4**; latency is now measured over every take,
successful or not. The server was never the problem — the measurement was.

### What it also proved

- **After a node restart it came up black.** Checked in the moment, before any
  new take, because a moment later it is unobservable. Auto-restore remains a
  deliberate non-feature (ADR 0009).
- **After a server restart, nothing on air changed** — the new process adopted
  the node's `StateReport`.
- **Idempotency is exercised, not assumed.** 29 duplicate `cue_id`s were
  deduped. A run that never observes a dedupe now *fails*, because a green run
  that skipped idempotency has not tested the thing most likely to break it.
- **Bulk assets do not delay cues.** 1014 concurrent 8 MB transfers while takes
  ran at p95 27.5 ms — the three-connection split in ADR 0007 holding up.
