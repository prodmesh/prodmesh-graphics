# S8 — Cue delivery under chaos

**Gates:** M2 · **Risk:** [R8](../RISKS.md#r8) · **ADRs:** 0007, 0008, 0009
**Status:** not started

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

## Result

*Not yet run.*

```
Date:
Server / node versions:
Takes fired:
Fired twice:
Lost:
Cues with no terminal outcome:
Take latency under asset saturation (p50 / p95 / max):
State after server restart:
State after node restart:
Gap-recovery correctness:
Notes:
```
