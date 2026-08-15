# ADR 0010 — Outputs, named layers, and key+fill

Status: accepted · 2026-08-14

## Context

An output almost never shows one graphic. A typical service has a bug in the
corner all morning, lower thirds coming and going underneath it, and occasionally
a fullscreen that covers both. So a node's output is a stack, and the question is
what the stack's slots are called and who gets to decide.

CasparCG answers with arbitrary numbered layers: any template on any layer, layer
numbers chosen by the operator or the automation. Maximum flexibility, and the
cost lands squarely on the person in the booth — every control surface, every
Companion button, and every conversation now involves a layer number that means
something only by convention. That is exactly the complexity ProPresenter avoids
and that the North Star asks us to avoid.

The wiring question is separate and equally opinionated. Keyed graphics reach a
switcher in several ways, and houses genuinely differ: fill and key on two SDI
outputs into an external keyer, a single NDI stream with alpha, a display output
into an HDMI input with no keying at all. Picking one would be wrong for most
rooms.

## Decision

### Named layers, configured per output

Each output carries an ordered stack of **named layers** — typically Bug, Lower
Third, Ticker, Fullscreen, bottom to top — defined in Configure. **Each layer
holds at most one live graphic.**

Taking a new lower third replaces what is on the Lower Third layer and does not
touch the Bug. That sentence is the whole operator model, and it is legible in
every context: a Companion button reads "Take · Lower Third," a RossTalk command
names a layer, the Show view lists layers by name with their occupant, and the
API takes a layer name rather than an index.

**`clear-layer` and `clear-all` are first-class cue operations**, not macros over
take-out. An operator who needs everything gone needs it gone now, not animated.

Layer names are per-output configuration, so a room with an unusual need can have
one — but the defaults are the defaults, and a volunteer never has to invent one.

### Outputs: 1–4 per node, configured independently

Each output declares:

- **Device and kind** — DeckLink, NDI, or GPU display.
- **Format** — resolution and exact rational frame rate.
- **Colour** — primaries, transfer, matrix, and range, named explicitly (ADR 0012).
- **Key mode** — one of: `fill`, `key`, `fill_key_pair` (two device ports), or
  `embedded_alpha`.
- **Layer stack** — the named layers, in order.
- **Audio mapping** — which layers' audio lands on which output channel pairs.

**Outputs are grouped by clock.** Outputs sharing a device — and therefore a
frame clock — form an output group with one render thread pumped by that group's
frame callback. Two references means two render loops. A vsync-driven display
output is never in a phase-locked group and is never claimed to be (ADR 0011).

For `fill_key_pair`, the reliable path is **two ports on the same device
scheduled at the same display time.** Same device means same clock, which is what
actually guarantees phase. Device capability varies wildly, so **probing is
mandatory** and Configure shows what each device actually reported rather than
what the model number suggests.

## Consequences

- The operator model is one sentence and no numbers.
- Control surfaces are self-documenting: a button labelled with a layer name
  needs no cheat sheet taped to the desk.
- Clear-layer and clear-all are obvious operations rather than compositions.
- Per-output key configuration means one node can feed an SDI keyer and an NDI
  receiver and a lobby display simultaneously, which is what rooms actually look
  like.
- **Two graphics cannot share a layer.** A design that wants a lower third and a
  matching sponsor tag animating together must author them as one template, or
  use two layers. That is a real constraint and it is the right one — it is what
  keeps "what is on the Lower Third layer" answerable.
- Output configuration is a lot of surface area, and it is where a room gets
  misconfigured. Hence the test patterns in Configure, and hence ADR 0012's
  insistence that they be *provable* rather than merely present.
- Output groups mean the node's threading model is determined by the plant's
  wiring, which is unusual and has to be documented plainly.

## Alternatives rejected

**Numbered layers, unrestricted (CasparCG model).** Maximum flexibility.
Rejected: it pushes the cost onto the operator and every control surface, which
is the specific trade the North Star refuses.

**One graphic per output.** Radically simple; a combined look is authored as one
template. Rejected — the first time the bug has to stay up while lower thirds
change, this collapses, and that is every service.

**Named layers plus "Looks" that fire several layers as one action.** Genuinely
how a real show gets driven, and probably right eventually. Deferred rather than
rejected: it is a composition over this model, not a change to it, and it belongs
after M2 has proved the layer model in a real service.

**Internal compositing over program video.** Simplest wiring. Rejected outright —
it puts the node in the program path, and a node failure would then be a lost
picture rather than a lost graphic. That is a completely different reliability
posture and not the one this system is designed for.

## Seam

Output configuration names primaries, transfer, matrix, range, and key mode
explicitly rather than implying them from a preset. That is what makes HDR, wider
gamuts, and SMPTE 2110 additive later rather than a schema break.

Layer stacks are data, so a future "Looks" feature, a template that spans layers,
or a per-room layer vocabulary are all configuration changes rather than
architectural ones.

The output backend interface (`pmg-out-*`) is a trait with three implementations
from day one, which is the cheapest possible insurance that a fourth — 2110, a
hardware compositor, a file writer — fits without disturbing the compositor.
