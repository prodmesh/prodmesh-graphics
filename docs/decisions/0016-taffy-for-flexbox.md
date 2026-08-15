# ADR 0016 — Taffy for flexbox, behind our own trait

Status: accepted · 2026-08-14

## Context

[ADR 0005](0005-the-pmg-scene-format.md) chose a flexbox box tree on the Yoga
model, because it is the layout model every designer already has in their head
from CSS and because content-sized boxes give the shrink-and-grow lower third
for free. That settled the *model*. It did not settle who implements it.

Flexbox is deceptively large. The specification has a long tail of behaviours —
`min-content` versus `max-content` sizing, flex-basis resolution against
indefinite containers, the interaction of `align-items: stretch` with an aspect
ratio, percentage resolution against a parent that is itself content-sized —
and almost none of it is visible until a designer builds the layout that hits
it. Writing it from scratch is not a week; it is a long tail of subtly wrong
output discovered one template at a time.

There is a sharper reason to care here than "it would take a while." Spike S1's
bar is **zero geometric displacement** between the native and browser builds.
Layout is the first stage of that comparison and the one most likely to move a
glyph. A layout engine that is subtly wrong is bad; a layout engine that is
subtly *differently* wrong under two build configurations would invalidate the
measurement that ADR 0001 rests on.

`taffy` is the mature Rust implementation of this model: pure computation, no
platform dependencies, deterministic, and it builds for `wasm32`.

## Decision

**Use `taffy` as the flexbox solver, behind `pmg-layout`'s own API.**

- `pmg-layout` exposes `solve(doc, data, measure) -> Solved` and the
  [`TextMeasure`] trait. **Taffy does not appear in the signature.** Callers see
  our node ids and our rectangles.
- The SceneDoc → taffy translation is one function, so the mapping from our
  `Dimension` to taffy's is in a single readable place rather than scattered.
- **`visible_if` is applied before the tree is built**, not after. A node that
  fails its condition is never inserted, so it takes up no space and contributes
  nothing to a content-sized parent — which is the entire point (ADR 0005 §5.3).
- Text measurement stays ours. `TextMeasure` is a trait so `pmg-layout` remains
  testable with a deterministic stub, and so the text stack can be replaced —
  spike S2 explicitly contemplates falling back to Skia for text — without the
  layout solver noticing.

## Consequences

- We get the flexbox long tail correct without owning it, and correctness here is
  load-bearing for S1's displacement bar.
- One more dependency in the **pure set**, which is the set with the strictest
  rules. Taffy qualifies: no I/O, no platform, no vendor SDK, and it compiles for
  `wasm32-unknown-unknown` — the required CI job is what enforces that, so a
  taffy release that broke it would fail the build rather than surprise us later.
- Determinism is inherited rather than guaranteed by us. Taffy is pure `f32`
  computation with no ambient state, so identical input gives identical output on
  both targets; S1's corpus is what actually checks it, and layout is compared as
  *numbers* before anything is rendered.
- The trait boundary means a future decision to replace taffy — or to add a
  constraint solver for the cases flexbox cannot express — is contained.
- A taffy upgrade can move a rectangle. Treat it like a wgpu bump: a
  release-gated change requiring a golden re-run, not a routine dependency
  update.

## Alternatives rejected

**Write our own flexbox.** Full control, no dependency. Rejected on the long
tail: the parts of flexbox that are hard are exactly the parts that stay
invisible until a designer hits them, and being subtly wrong in the layout stage
would undermine the one measurement ADR 0001 depends on. This is the same
reasoning that made a purpose-built *renderer* correct in ADR 0002 and makes a
purpose-built *layout engine* wrong: the renderer is where our differentiation
and our frame budget live, and flexbox is a solved algorithm with a published
specification.

**Yoga itself, through FFI.** The reference implementation. Rejected because it
is C++ and would put a vendor boundary and an `unsafe` surface inside the pure
set — the one part of the workspace that exists to have neither, and the reason
`pmg-render` can compile to WASM unchanged.

**A constraint solver instead (Cassowary).** Already rejected in ADR 0005 for
authoring reasons; nothing here reopens it.

## Seam

`TextMeasure` is the seam that keeps text replaceable — spike S2 may yet move
the text stack, and the layout solver must not care.

`pmg-layout`'s own API is the seam that keeps *taffy* replaceable. Nothing above
it names a taffy type, so swapping the engine, or adding a second layout mode for
something flexbox cannot express, is contained to one translation function.
