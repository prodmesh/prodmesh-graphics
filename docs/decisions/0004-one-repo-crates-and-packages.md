# ADR 0004 — One repo: the crate and package layout

Status: accepted · 2026-08-14

## Context

This system is a Rust workspace, an Express server, a React app, a WASM module,
and two vendor SDK bindings. The natural instinct is to split it: the node is a
different artefact from the web app, ships to different machines, and has a
different release cadence.

Three facts push the other way.

**ADR 0001's premise is that the renderer is the same code.** If the WASM module
and the node consume `pmg-render` from different repositories at different
versions, "these must never drift" becomes a version-matrix problem — at exactly
the point where drift is the failure mode the architecture exists to prevent.

**The scene format has four consumers**: the designer's WASM build, the node, the
server's validation, and `pmgctl`. They need one schema, one migration chain, and
one golden-image corpus. Four repositories means four opportunities for one of
them to be a version behind on a format change.

**Server and node ship as a matched pair.** A version skew between them is a
show-stopper, not a degradation.

## Decision

**One repository. A Cargo workspace and one npm project, side by side.**

```
crates/     17 crates — see docs/ARCHITECTURE.md for the table
sys/        decklink-sys, ndi-sys — vendor boundaries, all unsafe
server/     Express 5, plain ESM, no build step
src/        React 19 + TypeScript + Vite
testdata/   golden templates, golden PNGs, migration fixtures
```

Two structural rules come with it, and they are the reason the layout is what it
is:

**Rule 1 — `pmg-render` has no I/O and no platform.** It takes a `SceneInstance`,
a `wgpu::Device`, and a target texture. That signature is what lets it compile to
WASM unchanged.

**Rule 2 — the pure set may not depend on tokio, the filesystem, or any vendor
SDK.** The pure set is `pmg-scene`, `pmg-layout`, `pmg-text`, `pmg-anim`,
`pmg-render`, `pmg-color`. **Enforcement is a required
`wasm32-unknown-unknown` build job.** That job *is* the enforcement — a lint
would drift, and a build that fails cannot be ignored.

And one rule about the server that belongs here because it is a layout decision:

**The server never parses a SceneDoc.** It stores SceneDocs as opaque CAS blobs
plus a small JSON manifest sidecar that the Rust compiler emits — binding schema,
timeline names, asset hashes, `template_rev`, `format_version`, `min_reader` — and
validates the sidecar against a JSON schema and nothing more. The alternative is a
JavaScript reimplementation of the scene format, which is two implementations of
the thing that must never disagree. This is the same discipline that keeps
`arrangeSlides()` a single expansion in ProdMesh-Control.

## Consequences

- A format change lands in one commit across every consumer, and CI proves all of
  them still work.
- The parity job cannot be skipped or forgotten, because it is how the workspace
  is defined.
- `pmg-render` stays clean by construction rather than by review, which matters
  because the pressure to reach for a file handle or a `tokio::spawn` inside it
  will be constant.
- **CI is heavier**: Rust across macOS/Linux/Windows, plus the WASM target, plus
  Node, plus vendor SDKs that are not on every runner. Mitigated with
  path-filtered jobs — Rust jobs skip on `src/**`-only changes and vice versa.
- A single `git clone` is large, and a contributor who only wants to fix a React
  bug still gets the Rust workspace.
- Release tags cover everything at once, which is what we want for the
  server/node pair and slightly awkward for a designer-only fix.

## Alternatives rejected

**Polyrepo — node, renderer, web app separately.** Cleaner ownership, lighter CI
per repo. Rejected because it makes renderer drift *possible*, and this
architecture's central bet is that it is not.

**Renderer as a published crate consumed by both.** A middle path. Rejected
because it adds a publish step to the innermost development loop — changing a
layout rule and seeing it in the designer would mean cutting a release — and
because the version matrix returns the moment anyone pins an older one.

**Server-side scene validation in JavaScript.** Would let the server give better
errors on publish. Rejected as two implementations of one format. The compiler
emits the manifest; the server checks the manifest.

## Seam

The workspace layout is the seam for extraction. Because the pure set has no
platform dependencies and no I/O, `pmg-scene` + `pmg-layout` + `pmg-text` +
`pmg-anim` + `pmg-render` could be published as an SDK — the thing a third party
would embed to render our templates — without disentangling it from a server or a
node first.

`sys/` isolates every vendor relationship. `server/` and `src/` are replaceable
independently of the engine, and the API between them is documented in
`docs/specs/http-api.md` precisely so that stays true.
