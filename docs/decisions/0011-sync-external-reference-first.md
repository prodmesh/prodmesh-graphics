# ADR 0011 — Sync: external reference first, PTP later — what phase-lock does and does not promise

Status: accepted · 2026-08-14

## Context

Graphics that key over live video have to be in phase with it, or the switcher
sees a frame that arrives at the wrong moment and the result is a tear, a
dropped field, or a graphic that stutters on entry. That much is uncontroversial.

What is controversial — or rather, what is routinely misunderstood — is what
"in sync" buys you. Blackburst or tri-level to every node makes every node emit
frames at the same instants. It does **not** make them agree on what to call
those instants. Two nodes locked to the same generator, both counting frames
since they started, will be thousands of frames apart in their numbering, and
neither is wrong.

That distinction matters because the obvious next feature request — "take these
two graphics on two nodes at the same frame" — sounds like something genlock
already gives you. It does not, and a system that quietly pretends otherwise will
be off by a frame or two in a way nobody can reproduce.

The available hardware also constrains this. There is DeckLink with a reference
input and there is NDI. There is **no SMPTE 2110 plant and no PTP grandmaster** to
develop against, so anything built for them would be built blind.

## Decision

**Priority 0: external reference.** DeckLink genlock/ref input, or an NVIDIA sync
card for display outputs. Configure exposes the reference format and the lock
state, and surfaces loss of lock as a snapshot topic within seconds.

**Priority 1: PTPv2.** Designed for, not built. No hardware to verify against
means anything written now would be untested by construction.

**Priority 2: SMPTE 2110 and NMOS.** Not foreclosed — output configuration names
primaries, transfer, matrix, and range explicitly, and the output backend is a
trait — but not designed.

### What v1 promises, stated precisely

| Scope | What locks it | Promised? |
|---|---|---|
| Multiple outputs on one DeckLink device | Same scheduled display time on a shared device clock | **Yes** |
| Two DeckLink cards in one machine | Reference to both, from a DA | Yes, with the wiring |
| Across machines | Same reference to every node | Frame *timing* aligns |
| GPU display output | Nothing, absent a Quadro Sync card — and that locks to the display, not to SDI | **No** |

> **v1 promises phase-locked outputs *within* a node. It does not promise
> frame-accurate simultaneous takes *across* nodes.**

Say it in the UI, not only here. Configure shows a node's reference state; it
does not show a green light implying cross-node frame agreement.

### The architectural consequence

**One render thread per output group, pumped by that group's own frame callback.**
An output group is a set of outputs sharing a clock. Two references means two
render loops. A vsync-driven display output is never placed in a phase-locked
group and never described as locked.

### The primitive that ships unused

**`Cue` carries an `at: <frame|timecode>` field from day one**, even though v1
always sends "next available". It costs nothing now and is a protocol break
later. When cross-node scheduled takes are real, they will need timecode — LTC or
VITC from the same generator, read by DeckLink — plus this field, and the protocol
will already have room for it.

## Consequences

- Fill and key on one device are genuinely in phase, which is the case that
  matters most and the one that has to be exactly right.
- The system does not claim a guarantee it cannot keep, so nobody builds on a
  false one.
- Reference loss is visible in seconds rather than discovered by a confused
  operator watching a stuttering graphic.
- The node's threading model is determined by the plant's wiring, which is
  unusual enough to need explaining in Configure's help text.
- A room that wants graphics on a display output gets no phase guarantee at all,
  and Configure has to say so plainly rather than showing the same green tick.
- PTP and 2110 remain a design risk, deferred honestly: we will find out what
  they need when we have hardware, not before.

## Alternatives rejected

**PTP as P0.** More modern, and the direction the industry is going. Rejected
because there is no grandmaster and no 2110 plant here. Building an untestable
sync implementation and shipping it as "supported" is worse than not having it.

**Software sync — align on wall clock via NTP.** Rejected: NTP's accuracy is
orders of magnitude short of a frame, and a system that appears to sync and
doesn't is worse than one that admits it cannot.

**Claim cross-node frame accuracy and hope.** Rejected explicitly, because this
is the failure mode this ADR exists to prevent. It would work in every test and
fail on the one occasion it mattered.

**Free-run everything, let the switcher frame-sync.** Genuinely viable for some
plants, and it is what happens on the NDI path today. Rejected as the default
because it costs a frame or more of latency and gives up on clean animation
entry, but it remains the honest description of what an unreferenced output is
doing — and Configure says so.

## Seam

Naming the reference source, its format, and its lock state as explicit
configuration — rather than assuming an internal clock — is what makes PTP an
additional source rather than a redesign. The output-group abstraction already
models "these outputs share a clock," which is the concept PTP and 2110 both need.

The `at:` field is the seam for scheduled and cross-node takes, deliberately paid
for in advance.
