# S3 — DeckLink output from Rust

**Gates:** M0 → M2 · **Risk:** [R4](../RISKS.md#r4) · **ADR:** 0003
**Status:** enumeration half done (2026-08-14) · output path not started · **Highest-risk spike in M0**

## Question

Can we drive a DeckLink output from Rust, through a hand-written C shim, cleanly
enough for broadcast — scheduled playback, no late frames, reference lock, and
fill+key on two ports of one device?

This is the spike most likely to return bad news, and the one whose bad news is
most expensive to receive late.

## Method

A standalone binary — not integrated with anything — that:

1. Enumerates devices and reports full capabilities: supported modes, whether
   key+fill is available, which port pairs with which, whether a reference input
   exists and is locked.
2. Opens 1080p59.94, prerolls 3–5 frames, and runs scheduled playback driven by
   the completion callback.
3. Outputs a **moving 1-pixel vertical line** — the pattern where a dropped or
   late frame is unmistakable — for **ten minutes**.
4. Logs **every** `bmdOutputFrameCompletionResult`, especially
   `bmdOutputFrameDisplayedLate` and `bmdOutputFrameDropped`.
5. Repeats on internal clock and on external reference.
6. Repeats with fill+key on two ports of one device, scheduled at the same
   display time.

Structural constraints, applied from the start because they are the shipping
design:

- The shim is a C++ translation unit exposing a flat `extern "C"` API, compiled
  by the `cc` crate, **bound by hand against a hand-written header that is the
  contract of record**. No bindgen at C++ virtual interfaces. Same pattern as
  prodmesh-caption's `swift/include/pmcaption_speech.h`.
- **The completion callback allocates nothing, blocks on nothing, and takes no
  lock.** It pushes a token into a bounded channel and returns.
- Every interface pointer is wrapped in a Rust type whose `Drop` calls `Release`.

## Success bar

| Metric | Bar |
|---|---|
| Ten minutes at 1080p59.94 | **Zero** late frames, **zero** dropped |
| On external reference | Locks, and stays locked |
| Fill+key on two ports | Phase-identical (confirmed on a scope or by capture) |
| Per-frame CPU on the callback path | Bounded, no allocation |

Zero is the bar, not "few." One late frame in ten minutes is one visible glitch
per ten minutes of service.

## What to write down regardless of outcome

This spike's real output is facts for `docs/INTEGRATION-NOTES.md`, and they are
worth more than the pass/fail:

- Which of the devices on hand actually support key+fill on one device, and on
  which port pairs. **Model number does not predict this.**
- What preroll depth is actually needed at 1080p59.94 on this hardware.
- Which thread the completion callback really arrives on, and what the SDK
  documentation gets wrong about it.
- What the frame budget looks like in practice: how much of 16.68 ms is left
  after render and readback.
- Whether reference lock is reported reliably, and how fast loss is detectable.

## If it fails

- **Late frames under load** → investigate readback. A blocking buffer map on the
  render thread is the usual cause; async readback with a ring of staging buffers
  is the usual fix.
- **Callback threading is unworkable from Rust** → the shim absorbs it, marshalling
  to a Rust-owned thread. More shim, same architecture.
- **The SDK is fundamentally hostile to this approach** → ADR 0003 is revisited.
  The fallback is a C++ output process communicating with the Rust node over a
  local socket, which is ugly and preserves everything above it.

Note that M1 outputs NDI specifically so this spike's outcome does not block the
vertical slice.

## Result — enumeration half, 2026-08-14

The output soak has **not** run. What has: device enumeration and capability
probing, via `pmgctl decklink list|modes|ref`. Three findings, all in
`docs/INTEGRATION-NOTES.md` in full:

1. **The driver on the machine is 14.5; the SDK is 16.0.** SDK-16.0 IIDs do not
   exist in a 14.5 driver, so `QueryInterface` returns `E_NOINTERFACE` and every
   capability silently reads as its default — a Quad 2 presented as a card with
   no keying, no audio, and persistent id 0. Fixed with a newest-first IID
   fallback chain, which is safe **only** for methods at identical vtable slots.
   **`IDeckLinkOutput` diverges at slot 6**, so the output path below should
   narrow to one version and require a matching driver rather than extend the
   chain.
2. **External keying is reported on only 2 of 8 connectors** ([1] and [3]).
   Probed, not predicted by the model number — exactly what header rule 3 says.
3. **Connector-to-card grouping is unresolved.** Neither `persistent_id` nor
   `topological_id` is cleanly "the card"; both encode the sub-device in their
   low bits. **This spike must confirm on a scope that a fill+key pair really
   shares a clock before Configure offers one as phase-locked.**

Hardware for the soak: DeckLink Quad 2, 16 modes, 1080p59.94 available as
`60000/1001`. Reference input present but **not locked** — nothing feeding it, so
the soak needs a generator for its on-reference leg, and S4 is blocked until
there is one.

A prior number worth carrying in: a CPU-generated 1080p frame took **17.55 ms
against a 16.68 ms budget** over the NDI path. If a memcpy-and-encode path is
already at 105% of budget, DeckLink's scheduled output has no headroom for one —
the render and readback have to be on the GPU.

```
Date:
Hardware (device model, PCIe/Thunderbolt, firmware):
OS and Desktop Video version:
Mode tested:
Late frames in 10 min:
Dropped frames in 10 min:
Reference lock behaviour:
Key+fill port pairs CONFIRMED ON A SCOPE:
Preroll depth required:
Callback thread observations:
Notes:
```
