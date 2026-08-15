# S1 — Render parity: native wgpu versus browser WebGPU

**Gates:** M0 → M1 · **Risk:** [R1](../RISKS.md#r1) · **ADRs:** 0001, 0002
**Status:** not started

## Question

ADR 0001's whole premise is that the designer and the playout node run the same
renderer, so what a designer sees is what goes to air. Does that hold at the
pixel level, and if not, by how much and in what way?

Run this **before writing a renderer**. If it fails on a triangle, that is a very
cheap thing to learn.

## Method

1. Minimal `pmg-render` capable of: a solid triangle, an antialiased rounded
   rectangle, and one shaped text block from a font loaded as bytes.
2. `pmgctl render-golden` renders a corpus offscreen to RGBA8 at design
   resolution, natively.
3. A headless-Chrome harness renders the same corpus via `pmg-wasm`, offscreen,
   same resolution, same font bytes.
4. Diff per pixel, reporting **two independent numbers**:
   - **Channel delta** — max and percentile distribution.
   - **Geometric displacement** — via edge detection and cross-correlation.

Those are reported separately on purpose. A glyph two pixels to the left is a
failure of the design promise; a shade one step off is a GPU backend difference
we have already accepted. One combined number hides the one that matters.

Controls applied throughout, because they are the mitigations we would ship
anyway:
- **MSAA off.** Sample patterns are unspecified across backends.
- All antialiasing done in-shader, analytically or with deterministic ordered
  supersampling.
- No reliance on sampler filtering defaults.
- Identical `wgpu` version on both sides.
- Fonts loaded as bytes on both sides. **No CSS fonts, ever.**

## Success bar

| Metric | Bar |
|---|---|
| Pixels within 1/255 | ≥ 99.9% |
| Max channel delta | ≤ 2/255 |
| **Geometric displacement** | **zero** |

Geometric displacement is the hard gate. Any nonzero value means layout, shaping,
or rasterization differs, which is the thing shared code was supposed to
guarantee, and it invalidates the promise rather than merely degrading it.

## If it fails

- **Colour/AA deltas above bar** → narrow the shader surface, remove any
  remaining fixed-function dependence, retest. If it still fails, restate ADR
  0001's promise as "identical layout and glyphs, approximate colour," and record
  the measured bound. Acceptable.
- **Nonzero geometric displacement** → this is the serious one. Bisect: layout
  only (compare resolved rectangles as numbers, no rendering), then shaping
  (compare glyph ids and advances), then rasterization. A difference in the first
  two is a bug in our code and is fixable. A difference in rasterization means the
  fallback in ADR 0001 — node-rendered design preview streamed to the browser —
  becomes the plan.

## Result

*Not yet run.*

```
Date:
Hardware / OS / GPU:
Browser and version:
wgpu version:
Corpus size:
Pixels within 1/255:
Max channel delta:
Geometric displacement:
Notes:
```
