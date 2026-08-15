# ADR 0003 — Rust for the playout node, with hand-written C shims for vendor SDKs

Status: accepted · 2026-08-14

## Context

The playout node is the process that must not fail. It runs unattended during a
service, holds the frame budget, and a crash in it is a black graphic in front of
a congregation. The rest of the ProdMesh family's native code is C++, which is
the obvious precedent and the reason to ask the question carefully.

Two things push away from C++ here.

First, ADR 0001's shared renderer needs a WASM target. C++ to WASM is possible
via Emscripten but it is a second toolchain, a second build matrix, and a much
worse story for the wgpu/WebGPU binding that the browser side needs.

Second, the failure mode. A memory error in a Qt settings dialog is a bug report.
A memory error in the process holding a genlocked output at 10:15 on a Sunday is
a different category of event. Rust does not make the node correct, but it removes
an entire class of ways for it to die that we would otherwise have to be
permanently vigilant about, in code that runs on a real-time callback thread where
vigilance is hardest.

Against that: every SDK this node must talk to is C or C++. The Blackmagic
DeckLink SDK is a COM-alike with `IUnknown` refcounting, `QueryInterface`, and
virtual interfaces you are expected to *implement*. NDI is a C API. ffmpeg is C.
Rust does not make those disappear; it puts a boundary around them.

## Decision

**Rust throughout the node, with hand-written C shims for vendor SDKs, and all
`unsafe` confined to `sys/`.**

- `pmg-node` and every crate above it are safe Rust. No `unsafe` outside `sys/`.
- **`sys/decklink-sys/` owns a C++ translation unit** exposing a flat
  `extern "C"` API over the SDK's COM-alike, compiled by the `cc` crate. It is
  bound **by hand, against a hand-written header that is the contract of record**.
  We do not point bindgen at C++ virtual interfaces; that path is misery, and the
  header-as-contract pattern is taken verbatim from prodmesh-caption's
  `swift/include/pmcaption_speech.h`, comment included.
- **`sys/ndi-sys/` loads the NDI runtime dynamically** rather than linking it, so
  redistribution stays a deployment question rather than a build-time commitment.
- Threading discipline at the boundary is explicit, because the SDK dictates it:
  the DeckLink completion callback arrives on DeckLink's own thread. **Never
  allocate, never block, never take a lock there** — push a token into a bounded
  channel and return. Every interface pointer gets a Rust wrapper whose `Drop`
  calls `Release`.
- Errors: `thiserror` in libraries, `anyhow` at binary edges, `tracing` for logs.

## Consequences

- The shared-renderer story in ADR 0001 works, which is the largest single
  benefit and the reason this is not a close call.
- The class of bug that produces a mid-service crash from a lifetime mistake is
  largely gone from everything except `sys/`, where it is concentrated,
  reviewable, and small.
- **`sys/decklink-sys` is real work and it is on the critical path.** Existing
  crates are thin and none covers scheduled playback plus reference plus key/fill
  plus embedded audio to the standard needed. We are writing it. Spike S3 is
  scheduled in M0 specifically so we learn how bad it is before anything depends
  on it. See R4.
- Fewer people can maintain the node than could maintain a C++ one, and the C++
  siblings' build tooling does not transfer.
- ffmpeg comes in through `rsmpeg`/`ffmpeg-next`, which is a real dependency with
  a licensing constraint (below).
- `cargo build` on three targets plus WASM is a heavier CI matrix than the C++
  siblings', mitigated with path-filtered jobs.

## Alternatives rejected

**C++ for the node.** Direct, unmediated access to every vendor SDK we will ever
want, matching the family's existing native code. Rejected because it forfeits the
WASM renderer target that ADR 0001 is built on, and because a segfault in this
particular process is a black screen on a Sunday. The SDK-access advantage is
real but it is bounded to `sys/`, which is exactly where the shim puts it.

**Rust supervisor with a C++ render core.** Keeps C++ for the GPU hot path.
Rejected because it puts an FFI boundary through the most performance-sensitive
seam in the system, and because the render core is precisely the part that must
also run in a browser.

**A vendor DeckLink crate from crates.io.** Rejected after review: thin,
incomplete, and none handles the callback-implementation side we need. Depending
on one would mean forking it within a month; better to own the shim from the
start, in the pattern the family already uses.

## Seam

The `sys/` boundary is the productization seam for every vendor relationship.
Each SDK's terms — Blackmagic's header licence, NDI's redistribution rules — are
confined to one directory with one hand-written header describing exactly what we
use. Swapping a vendor, or shipping a build without one, is a directory-level
change rather than an archaeology project.

**ffmpeg is LGPL components only, dynamically linked, no GPL.** Written down here
so nobody links `libx264` for convenience and quietly makes the whole binary
GPL. That would foreclose `docs/VISION.md`'s posture permanently.
