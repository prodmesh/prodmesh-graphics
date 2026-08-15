# ADR 0012 — Colour, range, and the key channel

Status: accepted · 2026-08-14

## Context

Colour bugs in a keying system are unusually expensive because they are hard to
see and easy to ship. A white lower third with a dark halo around the letters
looks fine on a confidence monitor and obviously wrong once the switcher's keyer
softens the edge over live video. By then it is Sunday.

There are five ways to get this wrong and they are all common:

1. **Compositing in gamma-encoded values.** Blending sRGB or Rec.709-encoded
   numbers is arithmetically meaningless, and it produces exactly the dark-halo
   artefact above.
2. **Putting the key through a colour matrix.** Fill is a colour and goes
   RGB→YCbCr through 601, 709, or 2020. The key is not a colour — it is a
   monotone alpha signal — and running it through a matrix subtly corrupts it.
3. **Legal versus full range.** Switchers expect narrow range; some plants want a
   full-range key. Getting it backwards clips highlights or crushes the key's
   soft edge.
4. **Premultiplied versus straight alpha.** DeckLink's embedded-alpha modes and
   NDI each have an opinion, and mixing them produces fringing.
5. **Assuming source media is 709.** It carries its own tags and often is not.

## Decision

### The internal model

**Render and composite in linear-light, premultiplied RGBA16F. Encode exactly
once, at the output stage.** No intermediate stage decodes to display-referred
values and re-encodes; every such round trip is a place for a rounding error and
a gamma mistake to hide.

### The key is not a colour

Fill goes RGB→YCbCr through the output's configured matrix.

**The key does not.** It is written as `Y = alpha`, mapped to the configured
range, with `Cb = Cr =` neutral (512 in 10-bit). No matrix, no primaries, no
transfer function.

This is the rule most likely to be violated by someone reusing the fill path
"because it already works," and it belongs in `AGENTS.md`'s gotchas as well as
here.

### Output configuration names everything explicitly

Every output declares `primaries`, `transfer`, `matrix`, and `range` as separate
named fields rather than a single preset like "HD". A preset is a guess; four
fields are a statement, and they are what makes HDR and wider gamuts additive
later rather than a schema break.

**Range is per-output and per-signal**: an output may emit narrow-range fill and
full-range key, because some plants want exactly that.

### It must be provable, not merely configured

Configure emits test patterns, and they are diagnostic instruments rather than
decorations:

- **A ramp with marked 0% and 100% points**, so an operator can confirm on a
  scope or a switcher's multiviewer that legal range is actually legal.
- **75% colour bars**, for matrix verification.
- **A soft-edged white-on-transparent text block**, which is the pattern that
  exposes premultiplication and AA-edge errors — the failure that only shows up
  over live video.

### Alpha convention

**Premultiplied internally. Convert at the boundary.** Each output backend
documents which convention its API wants and where the conversion happens.

### Media

Source media carries its own colour tags; **convert on decode, never assume
709**. **HDR-tagged source is rejected at ingest with a clear message** rather
than silently mangled.

### HDR

**v1 is SDR Rec.709.** Not attempted, and named explicitly enough in the config
struct that adding it is additive.

## Consequences

- Antialiased edges and soft keys are correct over any background, which is the
  whole point and is invisible until it is wrong.
- Colour behaviour is auditable: the output config says exactly what the signal
  is, and a test pattern proves it.
- Adding HDR, P3, or 2020 later is new values in existing fields.
- RGBA16F linear costs memory bandwidth versus 8-bit. Accepted — it is the only
  way to get the edges right.
- Every output backend needs an explicit conversion stage rather than handing a
  buffer to the SDK. More code, and the right code.
- Rejecting HDR media at ingest will annoy someone. The alternative — mangling it
  quietly — is worse.
- Test patterns are real work in M3 and they are not optional; ADR 0010's output
  configuration surface is only trustworthy because they exist.

## Alternatives rejected

**Composite in 8-bit display-referred sRGB.** Cheap and fast. Rejected: it is the
direct cause of failure mode 1, and the artefact appears exactly where it is most
embarrassing.

**Reuse the fill conversion path for the key.** Less code, one path. Rejected —
failure mode 2, and the reason the rule is stated so bluntly.

**A single "colour preset" per output (`SD` / `HD` / `UHD`).** Friendlier UI.
Rejected: it hides the four things that actually matter and makes any future
addition a schema change. The UI can *offer* presets that fill in the four
fields; the config stores the four fields.

**Straight (non-premultiplied) alpha internally.** Simpler to reason about in
isolation. Rejected: correct compositing of antialiased edges requires
premultiplication, and doing it at every blend instead of once is slower and more
error-prone.

**Attempt HDR in v1.** Rejected — no HDR plant, no HDR reference monitor, and no
way to verify. It would be a guess shipped as a feature.

## Seam

Naming primaries, transfer, matrix, and range as first-class fields is the seam
for HDR, wide gamut, and SMPTE 2110's colour signalling — all of which are new
values, not new structure.

`pmg-color` is a pure crate with no I/O, so it is testable numerically and
verifiable against captured hardware output (spike S9). Its results become dated,
hardware-attributed entries in `docs/INTEGRATION-NOTES.md`, which is the form a
colour claim has to take to be worth anything.
