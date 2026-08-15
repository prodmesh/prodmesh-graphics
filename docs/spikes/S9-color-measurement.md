# S9 — Colour and key, measured

**Gates:** M3 · **Risk:** [R9](../RISKS.md#r9) · **ADR:** 0012
**Status:** not started

## Question

Does the full path — linear premultiplied compositing, output encoding, colour
matrix, range mapping, key generation — produce numerically correct values on
real hardware, into a real switcher?

**This is a measurement spike.** Its output is not pass/fail; it is a dated,
hardware-attributed entry in `docs/INTEGRATION-NOTES.md`. A colour claim in any
other form is worth nothing.

## Method

Render each pattern through the complete pipeline to SDI at 709 narrow, capture
it back on a DeckLink **input**, and compare decoded values numerically to
expectation.

| Pattern | Expectation |
|---|---|
| **Ramp, 0% and 100% marked** | Marks decode at 64 and 940 (10-bit narrow), 0 and 1023 (full) |
| **75% colour bars** | Vectorscope boxes on target; matrix confirmed |
| **Soft-edged white text on transparent** | See §"the important one" |
| **Alpha staircase** (0, 25, 50, 75, 100%) | Key plane luma at the mapped values; **`Cb = Cr = 512` at every step** |

Then repeat for:

- `range: full` and `key_range: full` independently, to confirm they are genuinely
  independent.
- `embedded_alpha` on NDI, comparing against the SDI fill+key result.
- Each `matrix` value the hardware supports.

And finally, **through a real downstream keyer if one is available**: key the
soft-edge pattern over a bright background and over a dark one, and photograph
the glyph edges.

## The important one

**The soft-edged white-on-transparent text block, keyed over both a bright and a
dark background.**

This is the pattern that catches the classic broadcast CG bug. A
premultiplication error is invisible over one background and obvious over the
other, which is precisely why it survives internal testing and shows up on air.
Any dark fringe over a bright background, or bright fringe over a dark one, is a
failure.

The alpha staircase is the second most important, because it is what catches the
other classic: **the key having gone through a colour matrix.** If `Cb` or `Cr`
deviates from neutral at any alpha step, the key path is wrong.

## Success bar

| Metric | Bar |
|---|---|
| Ramp endpoints | Within ±2 code values of expectation |
| Colour bars | Within tolerance on a vectorscope |
| **Key chroma at every alpha step** | **Exactly neutral (512 / 512)** |
| Key luma at each step | Within ±2 code values |
| **Soft-edge fringing over bright and dark** | **None visible** |
| Narrow-range output | Nothing outside 64–940 |

The key-chroma row is the absolute one. Neutral means neutral.

## If it fails

- **Fringing** → premultiplication is wrong somewhere between the AA path and the
  output encode. Bisect with `pmg-color`'s unit tests, then with a raw texture
  dump before encoding.
- **Non-neutral key chroma** → the key is going through the fill path. This is
  the exact failure ADR 0012 §3 exists to prevent, and it means someone reused
  the fill conversion "because it already works."
- **Range endpoints wrong** → the range mapping is applied twice, or not at all.
  The ramp pattern with marked points is designed to make this unmistakable.
- **Bars off on a vectorscope** → wrong matrix for the format, most likely 601
  where 709 was intended.

## Result

*Not yet run.*

```
Date:
Hardware (output device, capture device, switcher/keyer):
Desktop Video version:
Format / colour config tested:
Ramp endpoints (measured vs expected):
Colour bars:
Key chroma at 0 / 25 / 50 / 75 / 100% alpha:
Key luma at each step:
Soft-edge fringing over bright / dark:
NDI embedded-alpha comparison:
Notes:
```
