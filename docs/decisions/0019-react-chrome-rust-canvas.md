# ADR 0019 — React keeps the chrome; the canvas and its geometry are Rust

Status: accepted · 2026-08-18

## Context

The template editor this project needs is not a form. It is drag-and-drop
placement of shapes, images and text; resize handles and snapping; build-in and
build-out animation with a keyframe timeline and editable curves. That is a
real design tool, and the question was asked directly: **does the UI need to
stop being React and become WebAssembly?**

The premise is already half-settled. ADR 0001 put **one shared Rust renderer**
behind both the node and the browser canvas, and `pmg-wasm`'s `DesignSession`
already paints the Design canvas with the same `pmg-render` the node runs. The
canvas is not React today and never was. So the real question is narrower than
"React or WASM": it is **where the boundary sits**, and the honest answer is
that today it sits in the wrong place.

`DesignSession` renders and exposes nothing else. It cannot say which node is
under a pointer, what a drag would do, or where a handle belongs. Anything that
needs those answers has to compute them in JavaScript — which means a second
implementation of the layout the Rust solver already owns.

## Decision

**Three layers, and the boundary is drawn by who owns the geometry.**

| Layer | Lives in | Why |
|---|---|---|
| Chrome — nav, forms, lists, rundown, inspector, layer tree | **React** | Text input, focus, keyboard, accessibility and screen readers are solved. Reimplementing them over a canvas is months of work to arrive back where the browser starts. |
| Canvas — pixels | **Rust → WASM** | ADR 0001. The designer sees what the node will output because it is the same code, and that guarantee is the premise of the whole product. |
| **Geometry — hit-testing, drag, snap, handle positions** | **Rust → WASM** | This is the change. It is layout knowledge, and the layout solver is in Rust. |

The third row is the decision. **Hit-testing in JavaScript would be a second
implementation of the thing that must never disagree** — the same argument that
keeps the server from parsing SceneDocs (ADR 0004) and keeps `arrangeSlides()` a
single expansion in ProdMesh-Control. A box's rectangle after flexbox, a
rotated node's true bounds, whether a click at (x, y) landed on a glyph or the
padding beside it: every one of those answers exists in `pmg-layout`, and
recomputing them in TypeScript means two answers that agree until they do not.

So `pmg-wasm` grows an interaction surface — `hit_test(x, y)`,
`begin_drag(node, handle)`, `drag_to(x, y)`, `snap_candidates()`,
`handles_for(node)` — returning coordinates. **React draws the handles at the
coordinates Rust reports** and owns everything with a text cursor in it.

### The keyframe editor is chrome, not canvas

A timeline is a list of tracks, each a list of keys with an easing. That is a
list widget, a number input and a curve preview — React and SVG. The *evaluation*
is already `pmg-anim`, and scrubbing calls it. Nothing about a timeline needs a
GPU surface, and building one there would put a text field inside a canvas for
no reason.

### What would change this

If profiling shows the WASM boundary crossing per pointer-move is the cost —
rather than layout, which is 0.3–2 ms — the answer is to batch a drag into one
call per frame, not to move the DOM into Rust. If that is not enough, revisit.

## Consequences

- `pmg-wasm` grows beyond `compile_template` and `DesignSession`. Its surface
  becomes an interaction API, and it needs the same care the wire contract gets.
- Hit-testing must be **exact**, not approximate: the same rects the renderer
  drew. A near-enough hit test is a designer clicking a box and selecting its
  neighbour, and it would be blamed on the UI rather than on the maths.
- Drag state has two homes — the gesture in React, the geometry in Rust — and
  the split has to be documented or it will be re-merged by someone tidying up.
- Undo/redo operates on the **SceneDoc**, not on pixels, and therefore lives
  wherever the document does. That is a separate decision and M5 work.
- React stays. No rewrite, no second framework, and the existing four modes keep
  working while Design deepens.

## Alternatives rejected

**Rewrite the UI in WASM (egui, Dioxus, Leptos).** The canvas is already Rust;
this would only move the *chrome*. It would cost text input, IME, focus rings,
screen-reader support and browser autofill — and buy nothing, because the part
that must be pixel-exact is not the chrome.

**Hit-test in TypeScript against a mirrored rect list.** Tempting, since
`Solved` already carries every rectangle and could be serialised. It is the
trap: it works until a rotation, a clip, or a glyph-level hit is needed, and by
then two implementations exist and disagree only in the cases nobody tests.

**A DOM overlay of absolutely-positioned divs per node.** Fine for a handful of
boxes and wrong at the first rotation or clip, because CSS transforms and the
renderer's transform stack are not the same maths. It also fights the compositor
on every drag.

## Seam

The interaction API is **pure geometry over a document** — it takes a point and
returns a node, or takes a drag and returns rects. It does not know about
React, the server, or a session. That is what lets the same functions serve a
native editor later, or a tablet surface, without any of them being a port.

Related: ADR 0001 (one shared renderer), ADR 0004 (the server never parses
SceneDocs — the same rule, applied to a different consumer), ADR 0005 (the scene
format), ADR 0016 (taffy as the solver).
