# Spec — Output configuration

**Normative.** Decisions and reasoning:
[ADR 0010](../decisions/0010-outputs-layers-and-key-fill.md),
[ADR 0011](../decisions/0011-sync-external-reference-first.md).

## 1. Shape

A node has 1–4 outputs. Each is configured independently:

```jsonc
{
  "id": "out-0",
  "name": "Program Key",
  "device": { "kind": "decklink", "index": 0 },   // decklink | ndi | display
  "mode": { "width": 1920, "height": 1080,
            "fps": { "num": 60000, "den": 1001 }, "interlaced": false },
  "key_mode": "fill_key_pair",   // fill | key | fill_key_pair | embedded_alpha
  "paired_port": 1,               // fill_key_pair only
  "color": {                      // §4 — four fields, never a preset
    "primaries": "bt709", "transfer": "bt1886",
    "matrix": "bt709", "range": "narrow",
    "key_range": "narrow"
  },
  "alpha": "premultiplied",       // premultiplied | straight (at the boundary)
  "reference": { "source": "external", "expect": "1080i5994" },
  "layers": [                     // §3 — bottom to top
    { "name": "Bug" }, { "name": "Lower Third" },
    { "name": "Ticker" }, { "name": "Fullscreen" }
  ],
  "audio": { "pairs": [ { "pair": 0, "layers": ["Fullscreen"] } ] },
  "restore_on_boot": false        // §6
}
```

## 2. Frame rate is exact and rational

`fps` is always `{num, den}`. **`60000/1001`, never `59.94`.** Timeline time is
computed from it directly:

```
t_ms = frames_since_start * 1000 * fps_den / fps_num
```

so an 800 ms `In` looks identical at 25p and at 59.94p. A float frame rate
accumulates error over a service and there is no reason to accept it.

## 3. Layer stacks

An ordered list of **named** layers, bottom to top. **Each holds at most one live
graphic.**

Defaults: `Bug`, `Lower Third`, `Ticker`, `Fullscreen`. A room may configure its
own, but the defaults are the defaults and a volunteer never has to invent one.

Names are unique per output and are how every surface addresses a layer — the
Show view, Companion, RossTalk, GPI, and the API. **Never by index.**

`clear_layer` and `clear_all` are first-class operations, not macros over
take-out: an operator who needs everything gone needs it gone now, not animated.

## 4. Colour — four fields, never a preset

| Field | Values |
|---|---|
| `primaries` | `bt601_525`, `bt601_625`, `bt709`, `bt2020`, `p3d65` |
| `transfer` | `bt1886`, `srgb`, `linear`, `pq`, `hlg` |
| `matrix` | `bt601`, `bt709`, `bt2020_ncl` |
| `range` | `narrow` (64–940 @10-bit) \| `full` |
| `key_range` | independent of `range` — some plants want a full-range key |

The UI may *offer* presets that fill these in. The config stores the four fields,
because a preset is a guess and four fields are a statement — and because that is
what makes HDR and wider gamuts additive rather than a schema break.

**v1 is SDR Rec.709.** `pq` and `hlg` are named so the field does not need
changing later; setting them is rejected in v1.

### 4.0 Who applies it

The four fields mean something only where the NODE encodes YCbCr. An SDI
output with `ycbcr: true` sends 10-bit 4:2:2 `v210` encoded by the node with
these settings applied (ADR 0012). Without it the card converts 8-bit RGB with
its own matrix and range, and these fields change nothing on the wire. NDI
carries RGB and the receiver converts; `ycbcr` is refused there. Full range is
clamped to 4–1019 on SDI, because 0–3 and 1020–1023 are timing-reference codes.

### 4.1 The key is not a colour

Fill goes RGB→YCbCr through `matrix`.

**The key does not.** It is written as `Y = alpha` mapped to `key_range`, with
`Cb = Cr =` neutral (512 in 10-bit). **No matrix, no primaries, no transfer
function.**

Full reasoning and the failure it prevents: `color-and-key.md` §3.

## 5. Key modes and wiring

| `key_mode` | Wiring | Notes |
|---|---|---|
| `fill` | One output, RGB | No key; for a display or a non-keyed feed |
| `key` | One output, alpha as luma | The other half of a manual pair |
| `fill_key_pair` | **Two ports on one device**, same scheduled display time | The reliable path — same device means same clock, which is what actually guarantees phase |
| `embedded_alpha` | One output with alpha | NDI with alpha; DeckLink modes where supported |

**Capability is probed, never assumed.** Configure displays what the device
actually reported — supported modes, whether key+fill is available, which port is
its pair — rather than what the model number suggests. Device capability varies
wildly within a product line.

## 6. Reference and output groups

```jsonc
"reference": { "source": "external" | "internal", "expect": "1080i5994" }
```

**Outputs that share a device clock form an output group, and each group gets one
render thread pumped by its own frame callback.** Two references means two render
loops. The node's threading model is therefore determined by the plant's wiring,
which is unusual enough that Configure's help text says so.

**A `display` output is never placed in a phase-locked group and is never shown
as locked**, absent a sync card. Configure shows it as free-running, in words.

Loss of lock is published on `node:<id>:sync` within seconds and shown in
Configure as *"Booth-1 lost reference. Outputs are free-running."*

> v1 promises phase-locked outputs **within** a node. It does not promise
> frame-accurate simultaneous takes **across** nodes
> ([ADR 0011](../decisions/0011-sync-external-reference-first.md)).

`restore_on_boot` defaults to **false**: an unattended machine re-firing a
graphic after a power cut is a bad surprise.

## 7. Audio

Video layers may carry audio. `audio.pairs` maps layers onto output channel
pairs; DeckLink carries up to 16 channels.

**The video output is the clock. Audio resamples to fit.** The decoder is *pulled*
by the output callback, never pushed. A fixed offset relative to the video frame
is configurable per output for plant delay.

On decoder underrun: **hold the last video frame, log it, never stall the
output.**

## 8. Test patterns

Configure can put a pattern on any output. These are diagnostic instruments, not
decoration — they are the only way an operator can *prove* a configuration rather
than trust it.

| Pattern | Proves |
|---|---|
| **Ramp with marked 0% / 100%** | `range` and `key_range` are actually legal |
| **75% colour bars** | `matrix` and `primaries` |
| **Soft-edged white text on transparent** | Premultiplication and AA edges — the failure that only appears over live video |
| **Moving 1-px vertical line** | Frame timing, tearing, dropped frames |
| **Identify** | Large output name and node name — which cable is this? |

The soft-edge pattern is the important one. It is the pattern that catches the
classic broadcast CG bug, and it is why `docs/TESTING.md`'s manual checklist asks
for it over both a bright and a dark background.
