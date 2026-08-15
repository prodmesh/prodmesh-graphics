# S2 — Text and typography at broadcast quality

**Gates:** M5 · **Risk:** [R3](../RISKS.md#r3) · **ADR:** 0002
**Status:** not started

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

## Result

*Not yet run.*

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
