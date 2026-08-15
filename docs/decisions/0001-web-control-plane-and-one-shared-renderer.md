# ADR 0001 — A web control plane and one shared Rust renderer

Status: accepted · 2026-08-14

## Context

The four things this system does for people — Configure, Design, Edit, Show —
have to live somewhere. The ProdMesh family has two precedents and they point in
opposite directions. ProdMesh-Caption and ProdMesh-RTA are C++/Qt desktop apps.
ProdMesh-Control is a React front end on an Express server, and it is the one
multiple people use at once.

Graphics needs both things those precedents are good at. Design wants a real
editing surface: a canvas with layers, snapping, a keyframe timeline. Edit and
Show want to be open on a booth screen, a laptop, and a tablet simultaneously,
with an operator who can be anywhere on the LAN.

The problem that actually decides it is **preview fidelity**. If the design
canvas is drawn by one piece of code and the playout node by another, they will
disagree — about where a line wraps, about what the kerning does at that size,
about how a timeline eases. Every broadcast graphics system has this bug, and it
is the most expensive kind, because the designer only finds out on air. Xpression
and Viz solve it by making the designer and the renderer the same program. We
cannot do that if the designer is a browser and the renderer is a native process.

Unless the renderer is the same code compiled twice.

## Decision

**A web control plane, and one renderer library compiled to two targets.**

- The control server is Express 5 in plain ESM JavaScript on SQLite, and the UI
  is React 19 + TypeScript + Vite — ProdMesh-Control's stack, verbatim, down to
  the design tokens and the no-CDN rule. Multi-user, remote access, and a booth
  screen that is just a browser all come free, and the operational knowledge
  transfers between the two apps.
- **The renderer is a Rust library.** `pmg-render`, and the pure crates beneath
  it, compile to the native playout node *and* to WASM with WebGPU for the Design
  canvas. Same layout solver, same text shaping, same glyph rasterization, same
  binding evaluation, same timeline math.
- `pmg-render` therefore takes a `SceneInstance`, a `wgpu::Device`, and a target
  texture, and does no I/O and nothing platform-specific. Everything else lives
  above it. This is an invariant, not a style preference, and it is enforced by a
  required `wasm32-unknown-unknown` CI job rather than by a lint, because a lint
  would drift and a failing build cannot.
- **Preview and program both come from the node**, rendered by the same GPU with
  the same fonts, with preview encoded low-res and streamed to the Show view. The
  WASM build serves the *design* canvas; it is not what an operator confirms
  against before a take.

## Consequences

- The entire class of "it wrapped differently in the designer" bugs is
  structurally eliminated, which is the single most valuable property of the
  design.
- Multiple people can be in Configure, Edit, and Show at once, from any machine
  on the network, with no install.
- Every pure-set crate now has two consumers with different constraints. WASM has
  no threads without COOP/COEP (which conflicts with a plain-HTTP LAN appliance),
  lower GPU limits, and no filesystem. That cost is paid continuously, in every
  design decision inside those crates.
- The Design canvas is harder to build than a native one would be. Drag ergonomics
  in a browser canvas are real work, and jsdom cannot test them — so the canvas
  must be fully operable without a drag, and the gestures are hand-verified.
- **The promise must be stated honestly.** Shared code guarantees identical
  layout, glyphs, bindings, and timing. It does *not* guarantee identical pixels:
  native Metal and the browser's WebGPU differ in MSAA patterns, shader float
  precision, and sRGB handling. See R1 and spike S1. Never enable MSAA, do all
  antialiasing ourselves, and pin one wgpu version across both targets.
- If S1 fails badly, this ADR is the one to revisit — the fallback is a
  node-rendered design preview streamed to the browser, which costs latency and a
  node per designer but restores exactness.

## Alternatives rejected

**C++/Qt native app**, matching the C++ siblings. Best design-canvas ergonomics,
and it could link the renderer directly. Rejected because it makes the system
single-user-per-machine, puts project files on a share or in git, gives the
remote operator nothing, and adds a second UI stack to the family. The Design
advantage is real but it is the smaller half of the work.

**Qt designer plus a web operator view.** Each audience gets the right tool.
Rejected as two UI codebases and two release pipelines for one product, with the
fidelity problem still unsolved unless the renderer is shared anyway — at which
point the browser can have it too.

**Browser UI with a node-rendered design preview over WebRTC.** Perfect fidelity,
no WASM work. Rejected as the default because interactive drag latency against a
network round trip is a poor authoring experience and every designer needs a node.
Kept as the fallback if S1 refutes the parity bet.

## Seam

The renderer being a library rather than an application is what keeps every later
option open: a native designer, an embedded SDK, a headless render farm, a
CLI-driven batch renderer. Nothing above `pmg-render` may reach into it in a way
that assumes a particular host.

The server-to-UI boundary is REST and SSE over documented endpoints, so the React
app is replaceable without touching the server — which is the same seam that lets
ProdMesh-Control, Companion, and a DashBoard panel be clients of the same API.
