# S1 — Render parity: native wgpu versus browser WebGPU

**Gates:** M0 → M1 · **Risk:** [R1](../RISKS.md#r1) · **ADRs:** 0001, 0002
**Status:** **PASSED on Metal** 2026-08-14 · cross-backend (D3D12/Vulkan) untested

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

## Result — PASSED on Metal, 2026-08-14

```
Date:                    2026-08-14
Hardware / OS / GPU:     MacBook Pro M3 Max, macOS 26.3, Apple M3 Max (Metal)
Browser and version:     Chrome 148.0.7778.280 (Electron 42.7.0)
Browser adapter:         vendor "apple", architecture "metal-3"
wgpu version:            30.0.0 (identical both targets)
Corpus size:             3 figures at 512x512
Pixels within 1/255:     100.0000% on all three
Max channel delta:       0/255 on all three
Geometric displacement:  0.000000 px on all three
Alpha mass delta:        0.0000% on all three
```

**Bit-identical**, not merely within tolerance.

### The caveat that belongs with the number

Chrome's WebGPU reported `vendor: apple, architecture: metal-3` — **Dawn is
targeting the same Metal driver on the same GPU as wgpu-native.** So this is not
evidence that wgpu and Dawn agree across backends. It is evidence that two
independent WGSL frontends (naga and Dawn's) produce identical results on one
driver, and that nothing in our pipeline accidentally depends on
backend-defined behaviour.

**ADR 0001's promise should be quoted as "measured on Metal"** until the corpus
runs on Windows. A Windows designer against a Mac node is a real deployment
shape and is currently untested.

### The harness was validated by making it fail

A perfect score on a first run is a reason for suspicion. The corpus was
perturbed deliberately, and the two metrics separated exactly as intended:

| Perturbation | Channel delta | Centroid shift | Diagnosed as |
|---|---|---|---|
| Triangle, +3/255 red | **3/255 FAIL** | 0.000000 | colour differs, geometry did not |
| Rect, shifted 1 px | 252/255 | **1.000000 px FAIL** | geometry moved |

### What made it come out clean

The determinism rules in `crates/pmg-render/src/parity.wgsl`, each of which
exists because breaking it would cause a difference *by specification*:

- **No `fwidth`/`dpdx`/`dpdy`.** Derivative granularity is implementation-defined.
  Pixel scale arrives as a uniform instead.
- **No MSAA.** Sample patterns are unspecified across backends.
- **`Rgba8Unorm`, never `Rgba8UnormSrgb`** — no automatic colour conversion.
- **Every sampler field stated**; no filtering defaults.
- Raw RGBA out of the browser, **never through a canvas** — `toDataURL` may
  apply a colour profile, which would measure Chrome's colour pipeline instead
  of our renderer.

### Still to do

1. **Run the corpus on Windows (D3D12)** before ADR 0001's promise is stated
   without qualification.
2. Extend the corpus as `pmg-render` grows — this is now a permanent CI job,
   not a one-off.
