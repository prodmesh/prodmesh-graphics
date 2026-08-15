# S2 — Text and typography at broadcast quality

**Gates:** M5 · **Risk:** [R3](../RISKS.md#r3) · **ADR:** 0002
**Status:** **RUN 2026-08-15** — 4 of 5 measured criteria pass; **shudder FAILS and its root cause is open**

## Question

Is a Rust text stack good enough for broadcast CG — specifically when text is
**animated**, which is when the failures show?

Text is most of what a CG system draws, and owning text layout is the largest
single cost of ADR 0002. This spike establishes whether the chosen stack clears
the bar and what it costs per frame.

## Method

**Stack under test:** `parley` + `swash` + `fontique`, with `harfrust`/`rustybuzz`
underneath. (`cosmic-text` as a comparison point: pragmatic and it works, but its
API is text-editor-shaped, its line breaking is simplistic, and it bakes in a
system font DB — all three fight us here.)

A torture corpus: **20 strings × 5 faces × 3 sizes.**

Strings must include: Latin with kerning pairs and ligatures, all-caps, tabular
digits (a clock and a score), CJK, Arabic, Hebrew, combining diacritics, emoji
(COLRv1), and a mixed-direction string.

Then, the part that matters:

1. **Render each at 60 subpixel positions** across one pixel, animated, and
   inspect for shudder at 400% zoom and in motion.
2. Render every case **over a mid-grey and over a saturated background, through
   the key path**, and inspect the AA edges for dark or bright fringes.
3. Render natively and in WASM and diff (S1's harness).
4. Measure **glyph-cache miss cost per frame** with a full-screen ticker at
   1080p59.94 — the worst case for cache churn.
5. Verify OpenType features apply: `kern`, `liga`, `smcp`, `ss01`, and especially
   **`tnum`** — clocks and scores jitter without tabular figures.
6. Pin down vertical metrics: compare OS/2 typo, hhea, and win across the five
   faces and **choose one set**.

## Success bar

| Metric | Bar |
|---|---|
| Animated horizontal motion | **No visible shudder.** Fractional-offset caching at ⅛ px phases, or SDF |
| AA edges over a key | **No fringing** over bright or dark backgrounds |
| Glyph cache, full-screen ticker | < 2 ms/frame steady-state |
| `tnum` | Digits do not change advance width |
| CJK line breaking | Correct per `icu_segmenter`; visually checked by someone who reads it |
| Bidi | Correct run ordering for the mixed string |

**Subpixel positioning is the one most likely to make the output look amateur.**
Integer-snapped glyphs visibly shudder when text slides, and broadcast CG slides
text constantly.

**Never subpixel-RGB antialiasing** — it is meaningless over an alpha channel.
Grayscale only, premultiplied correctly.

## The decision this spike must produce

**One vertical metric set, documented, never to be changed.** Changing it later
moves every existing template by two pixels, silently. Write the choice into
`docs/specs/scene-format.md` and into `INTEGRATION-NOTES.md` with the comparison
data behind it.

Also decide explicitly: **sbix and SVG-in-OpenType emoji are unsupported.**
COLRv1 via swash is supported. Half-supporting a colour font format is worse than
declaring it out.

## If it fails

- **Shudder** → SDF glyph rendering. More work, better result, and it also helps
  scaling.
- **Fringing** → the premultiplication is wrong somewhere in the AA path. This is
  *the* classic broadcast CG bug and it is a bug in our code, not the stack's.
- **Cache thrash on a ticker** → a larger atlas, or pre-rasterizing the string
  once and animating the transform (which is the cheap path anyway, per ADR 0005).
- **The stack is inadequate** → Skia for text only, behind the `TextMeasure` and
  rasterization traits. This is why those are traits.

## Result — 2026-08-15

Run with `pmgctl spike s2 --font <ttf>` on a MacBook Pro M3 Max, macOS 26.3,
wgpu 30, Arial. **The spike turns three of the hazards into numbers rather than
opinions**, which is the part worth keeping regardless of the verdict.

| # | Criterion | Result | |
|---|---|---|---|
| A | Shudder — centroid linearity | **0.33 px** (text), **0.21 px** (box) | ❌ **FAIL** |
| C | Fringing over a key | 1/255 worst, over bright **and** dark | ✅ pass |
| D | Tabular figures | 0.000 px spread | ✅ pass |
| E | Script coverage | CJK and emoji `.notdef` | ⚠️ gap |
| F | Glyph cache, ticker scale | 0.055 ms warm, 1.12 ms cold | ✅ pass |

### A — shudder FAILS, and the cause is NOT what R3 assumed

**How it is measured.** Slide the text by a known sub-pixel amount; the ink's
centre of mass should move by exactly that amount. Deviation from a straight
line is the shudder, in pixels. This replaces "inspect at 400% zoom" with a
number that can gate a regression.

**What it found.** A staircase with a 1.0 px period. The centroid barely moves
for half a pixel, jumps ~0.44 px, then tracks correctly:

```
dx      centroid    delta
0.000   255.5000    —          ← exact
0.452   255.7697    0.052      (0.27 px moved over 0.45 px asked)
0.516   256.2060    0.436      ← JUMP, exactly at the half-pixel
1.032   256.5093    0.019
2.000   257.5000    0.021      ← exact
```

Whole-pixel motion is **perfect** (255.500 → 257.500 over 2.0 px). Sub-pixel
motion is lumpy, and the discontinuity lands exactly where an edge crosses a
pixel centre.

**The control changes the diagnosis.** An antialiased **box** — analytic SDF, no
texture sampling at all — stutters the same way (0.21 px). So this is **not**
glyph-atlas sampling, and it is **not** specific to text. It is the geometry or
coverage path, and it affects every animated element.

That matters because R3 frames shudder as a *text* problem solved by sub-pixel
glyph caching. The measurement says otherwise: sub-pixel caching governs
**within-run glyph placement**, while **motion smoothness comes from the
sampler and the coverage function**. Two different mechanisms; the brief
conflates them.

**A hypothesis, tested and disproved.** `smoothstep` is a cubic, and the exact
coverage of a pixel by a straight edge is *linear* in signed distance — so
point-sampling a cubic seemed likely to make coverage respond non-linearly to
motion. Switching to `clamp(0.5 - d, 0, 1)` moved the number from **0.214 to
0.205 px**. Marginal. **Not the cause.** The linear ramp is kept because it is
the correct coverage function, not because it fixed anything, and the shader
says so.

**Root cause: still open.** What is known: it is not the atlas, not text-specific,
not smoothstep, and whole-pixel motion is exact. Next candidates, in order —
8-bit alpha quantisation of a 1-px AA band; the AA band being too narrow to
carry sub-pixel information across a pixel centre; and the centroid metric
itself saturating when an edge column clamps at 0 or 255.

**What is NOT yet known: whether 0.2 px is visible.** At a typical reveal speed
(~1.1 px/frame) it is roughly 18% positional jitter, which could well read as
judder — or could be invisible. **That still needs eyes on a real output**, and
the metric now exists to tell whether any fix actually helped.

### C — fringing is clean

Worst deviation from a CPU reference composite is **1/255 over both a bright
(0.90) and a dark (0.05) background**, across 804 antialiased edge pixels. That
symmetry is the point: a premultiply error deviates *differently* over each, so
matching to a single code value on both says the premultiplied path is right.

Hazard 2 from R3 is clean, and the atlas-stores-coverage-only rule is doing its
job.

### E — no font fallback, and that is a real gap

Arial covers Latin, Arabic, Hebrew, and combining marks. CJK and emoji return
**`.notdef`** — tofu.

That is correct behaviour for a face that lacks the script, and it is why fonts
are per-template assets. But **there is no fallback and no validation**: a
template whose text needs a script its fonts do not carry will silently render
boxes. Validation should refuse that at publish, the way it already refuses a
mistyped field reference. **Recorded as an M5 gap.**

### F — the cache is not the problem

A 180-glyph full-screen ticker: **1.12 ms** to rasterize every glyph at every
phase (a one-off at arm), **0.055 ms** warm, 96 KB of atlas. A take does not
touch the cache per frame at all.

---

## Original brief follows

```
Date:
Hardware / OS:
Crate versions (parley / swash / fontique / harfrust):
Shudder at ⅛ px phases:
Fringing over bright / dark:
Ticker glyph-cache cost:
tnum verified:
CJK breaking reviewed by:
Vertical metric set chosen:
Native/WASM diff:
Notes:
```
