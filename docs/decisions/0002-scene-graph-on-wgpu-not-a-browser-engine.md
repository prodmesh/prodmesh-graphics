# ADR 0002 — A scene graph on wgpu, not a browser engine

Status: accepted · 2026-08-14

## Context

Something has to turn a template plus data into pixels, sixty times a second,
without ever being late. There are three families of answer, and the industry has
working examples of each.

**Embed a browser engine.** CasparCG's HTML producer and Singular.live do this.
Templates are HTML, CSS, and JavaScript, which means responsive layout,
world-class typography, animation, and an enormous authoring ecosystem all arrive
for free on day one. It is by a wide margin the fastest route to a good-looking
lower third.

**Use a 2D graphics library.** Skia, backed by an own scene graph. Excellent
vector quality and text layout, a big head start on the hardest parts.

**Build a scene graph on a GPU API.** What Xpression and Viz do. Everything is
ours: layout, text, animation, compositing, colour.

The North Star is explicit that this system should have "the authoring stability,
performance, and maturity of Ross Xpression or Vizrt." That phrase is what
decides it, and it is worth being precise about why, because the browser option
is genuinely tempting.

A Chromium instance is 200–400 MB of resident memory per output. It garbage
collects, on its own schedule, in the middle of your 16.68 ms frame budget. Its
compositor is designed to drop frames gracefully rather than to hit them exactly —
which is correct for a web page and wrong for something genlocked to a house
reference. And it moves: a Chromium release cadence inside a system that has to
open a template unchanged in three years is the direct opposite of authoring
stability. We would be inheriting somebody else's release treadmill for our most
critical component.

## Decision

**A declarative scene graph rendered by a purpose-built GPU renderer on wgpu.**

- A Template compiles to a **SceneDoc**: a box tree with layout, transform, and
  style, plus a binding schema, plus named keyframe timelines. It is data, not a
  program. See ADR 0005 and `docs/specs/scene-format.md`.
- `pmg-render` is a wgpu render graph: `SceneInstance` in, RGBA16F linear
  premultiplied texture out. Deterministic, with no allocation on the frame path
  and no source of nondeterminism inside it.
- wgpu rather than raw Vulkan/Metal/D3D12 because it gives one codebase across
  macOS, Linux, and Windows *and* the browser — which is the mechanism ADR 0001
  depends on. Raw Metal would be faster to get right on one platform and would
  forfeit that entirely.
- We do our own antialiasing, analytically or with deterministic ordered
  supersampling, and **never enable MSAA** — its sample patterns are not
  specified across backends and it is therefore a parity hazard (R1).
- Everything composites in linear premultiplied float and is encoded exactly once
  at output (ADR 0012).

## Consequences

- Frame timing is ours to hit or miss, with no third-party scheduler in the way.
  That is the point.
- Memory per output is measured in tens of megabytes, not hundreds, so four
  outputs on one machine is ordinary rather than heroic.
- Templates are documents. They cannot throw, cannot leak, and cannot depend on a
  network fetch at air time.
- **We now own text layout**, and text is most of what a CG system draws. This is
  the largest single cost of this decision — see R3 and spike S2. Subpixel glyph
  positioning, premultiplied AA edges over a key, OpenType features, bidi, and
  CJK line breaking are all now our problem. `parley` + `swash` + `fontique` is
  the chosen stack.
- **We own the responsive layout engine** too, which ADR 0011's flexbox choice
  makes tractable but not free.
- The authoring ecosystem is empty on day one. No Google Fonts picker, no CSS
  animation library, no Stack Overflow answer for "how do I make this bounce."
  Design mode has to be good enough to compensate, which is why M5 exists.
- **A wgpu version bump is a release-gated change requiring a full golden-image
  re-run** (R11), not a routine dependency update.

## Alternatives rejected

**Embedded Chromium (CEF).** Rejected on frame timing, memory, GC pauses, and the
release treadmill, as argued above. The authoring ecosystem is a genuine loss and
we are accepting it deliberately. If a template ever truly needs the web platform
— an exotic data widget, an embedded chart — the escape hatch is an HTML *layer
type* rendered off the critical path, and that would need its own ADR with an
honest account of what it does to the frame budget.

**Skia plus our own scene graph.** Seriously considered; it would remove most of
R3. Rejected because it gives less control over the GPU pipeline, makes zero-copy
paths to DeckLink and NDI harder, and — decisively — does not give us the browser
target that ADR 0001 needs. We would end up with Skia natively and something else
in the browser, which is the exact drift this architecture exists to prevent.

**A game engine.** Unreal or Godot as the renderer. Rejected: enormous
dependency, wrong determinism model, wrong text stack, and a licensing question
that collides with the posture in `docs/VISION.md`.

## Seam

The SceneDoc format is the seam, not the renderer. Because templates are declarative
data with a versioned schema, a future renderer — a different GPU backend, a
hardware compositor, a cloud rasterizer — can be written against the same
documents. The format is the asset customers would own; the renderer is an
implementation detail we can replace.

`pmg-render`'s I/O-free, platform-free signature is what keeps that true.
