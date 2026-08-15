# Spec — Colour, range, and the key channel

**Normative.** Decisions and reasoning:
[ADR 0012](../decisions/0012-color-range-and-the-key-channel.md). Implemented in
`pmg-color`, which is a pure crate and therefore numerically testable.

## 1. The pipeline, once

```
decode ──► linearize ──► LINEAR PREMULTIPLIED RGBA16F ──► encode once ──► output
  ▲                            ▲                              │
  │                            │                              ├─ fill: RGB→YCbCr via matrix
  media's own tags      all compositing,                       └─ key:  Y = alpha, NO matrix
  (never assume 709)    all animation,
                        all antialiasing
```

**One encode, at the output stage.** No intermediate decodes to display-referred
values and re-encodes. Every such round trip is somewhere a gamma mistake and a
rounding error can hide.

## 2. The working space

**Linear-light, premultiplied, RGBA16F.**

**Linear** because blending gamma-encoded values is arithmetically meaningless. It
is also the direct cause of the most common visible artefact in a CG system: a
dark halo around white text over a dark key, invisible on a confidence monitor
and obvious once a keyer softens the edge over live video.

**Premultiplied** because correct compositing of antialiased edges requires it,
and doing the multiply once beats doing it at every blend.

**16-bit float** because 8-bit banding shows in gradients and shadows, and
because linear 8-bit crushes the shadows badly. The memory bandwidth cost is
accepted; it is the only way the edges come out right.

## 3. The key is not a colour

> **Fill** is a colour. RGB → YCbCr through the output's configured `matrix`
> (601 / 709 / 2020), then to the configured `range`.
>
> **Key** is a monotone alpha signal. Write `Y = alpha` mapped to `key_range`,
> with `Cb = Cr =` neutral — **512 in 10-bit, 128 in 8-bit**. **No matrix. No
> primaries. No transfer function.**

This is the rule most likely to be violated by someone reusing the fill path
"because it already works." Running the key through a colour matrix mixes a
nonexistent chroma into the luma and shifts the alpha values slightly. The result
is a key whose soft edges are subtly wrong, which makes the downstream keyer's
soft edges wrong with it — and it is nearly impossible to see except on the
switcher, over real video, on the day.

It is stated bluntly here, in `AGENTS.md`'s gotchas, and in ADR 0012 for that
reason.

### 3.1 Reference implementation

```rust
/// Encode the key plane. The key is NOT a colour: it never goes through
/// a colour matrix. Doing so corrupts the soft edges in a way that only
/// shows up on the switcher's keyer, over live video, on the day.
fn encode_key_10bit(alpha_linear: f32, range: Range) -> (u16, u16, u16) {
    let a = alpha_linear.clamp(0.0, 1.0);
    let y = match range {
        Range::Narrow => 64.0 + a * (940.0 - 64.0),
        Range::Full   =>        a * 1023.0,
    };
    (y.round() as u16, 512, 512)   // neutral chroma, always
}
```

**Alpha is not gamma-encoded.** It is a coverage fraction and is mapped linearly
to the code-value range. Applying a transfer function to alpha is a bug.

## 4. Range

| Range | 10-bit | 8-bit |
|---|---|---|
| `narrow` | 64 … 940 (luma), 64 … 960 (chroma) | 16 … 235 / 16 … 240 |
| `full` | 0 … 1023 | 0 … 255 |

`range` (fill) and `key_range` are **independent**, because some plants want a
narrow-range fill and a full-range key.

Values outside the range are clamped, not wrapped. Configure's ramp pattern with
marked 0% and 100% points is how an operator confirms this on a scope, and it is
the only trustworthy way to know.

## 5. Alpha convention at the boundary

Internally **always premultiplied**. Conversion happens in the output backend,
which documents which convention its API wants:

| Backend | Wants |
|---|---|
| NDI (`UYVA` / `RGBA`) | Straight — unpremultiply at the boundary |
| DeckLink `bmdFormat8BitBGRA` embedded alpha | Straight |
| DeckLink separate fill + key | Fill is premultiplied over black; key is the alpha plane |

Getting this wrong produces fringing: dark fringes if you send premultiplied
where straight is expected, bright ones the other way. The soft-edge test
pattern (§7) is what catches it.

## 6. Source media

**Media carries its own colour tags. Convert on decode. Never assume 709.**

A 601-tagged SD clip dropped into a 709 template and passed through unconverted
has visibly wrong greens and reds, and it is the kind of thing that gets blamed
on the camera.

**HDR-tagged source is rejected at ingest** in v1, with a message naming the
transfer characteristic found, rather than being silently mangled. Someone will
be annoyed by this. Mangling it quietly is worse.

## 7. Test patterns

| Pattern | Proves | How to read it |
|---|---|---|
| **Ramp, marked 0% / 100%** | `range`, `key_range` | On a scope, the marks land at 64 and 940 (narrow) or 0 and 1023 (full) |
| **75% colour bars** | `matrix`, `primaries` | Vectorscope boxes |
| **Soft-edged white text on transparent** | Premultiplication, AA edges | Key it over a bright *and* a dark background. Any dark or bright fringe at the glyph edges is a bug. |
| **Moving 1-px vertical line** | Frame timing | Tearing or stutter is a dropped or late frame |

**The soft-edge pattern is the important one.** It is the only pattern that
catches the classic broadcast CG failure, and it must be checked over both a
bright and a dark background — a premultiplication error is invisible over one of
them.

## 8. Verification

`pmg-color` is pure, so its conversions are unit-tested against known values:
709 primaries round-trip, narrow/full range endpoints, the key path producing
neutral chroma at every alpha, and premultiply/unpremultiply round-tripping
within tolerance.

That certifies the arithmetic. It does not certify the plant. **Spike S9 renders
these patterns through the full path to SDI, captures them back on a DeckLink
input, and compares decoded values numerically to expectation** — and its results
become a dated, hardware-attributed entry in `docs/INTEGRATION-NOTES.md`, because
that is the only form a colour claim is worth anything in.

## 9. Not in v1

**HDR.** No HDR plant, no HDR reference monitor, and no way to verify. The config
fields are named (`pq`, `hlg`, `bt2020`) so adding it is new values rather than
new structure, and setting them is rejected today.

**Wide-gamut working space.** Rec.709 primaries throughout. `p3d65` and `bt2020`
are named for the same reason.
