# ADR 0017 — Playout mode: the playlist decides whether navigating is taking

Status: accepted · 2026-08-16

## Context

The original brief asked for "playout controls that vary by playlist". Two
concrete modes have since been named, and they behave almost oppositely.

**Lyrics.** Two lines of a song per slide. The operator follows the music on the
arrow keys, eyes on the band, not the screen. **Navigating IS taking** — there is
no arm, no confirm, no second action. Clicking or arrowing to a slide puts it on
air. Anything else is a beat late, and a beat late in a chorus is the whole
problem.

**The message.** Media-heavy, cued deliberately. **Arm, then take.** This is a
graphics cuelist in the Ross Xpression manner, and the stated objection to
ProPresenter is precisely that it lacks the arm step: the moment someone clicks,
it is out.

These are not two skins over one behaviour. The **cursor means a different
thing** in each — intent to arm versus a thing already on air. The **keyboard map
differs** — `↓` arms in one and fires in the other. **What "next" means differs.**
A design exploration made this concrete: the lyrics case wants a tile grid
crossed in two directions with a song-structure map, and the message case wants a
rundown rail with an inspector. The same widget cannot be both.

There is a hard constraint underneath. `POST /api/cue` is **synchronous** — it
does not return until the node confirms or the deadline expires — and that is not
a performance choice, it is ADR 0008's guarantee that the take button never lies.
Follow mode fires cues several times a second. An operator holding an arrow key
through a chorus would queue blocking HTTP requests behind each other.

So the question this ADR settles is not only "where does mode live" but "how does
Follow stay honest while staying fast".

## Decision

**Mode is a property of the Playlist.** Not the node, not the output, not the
rundown, and not a preference on the operator's screen.

Two modes in v1:

| Mode | Cursor means | Arrow keys | Take |
|---|---|---|---|
| `cue` | what I intend to arm | move the cursor | an explicit, separate action |
| `follow` | where I am in the content | move **and** take | implicit in navigation |

### The cursor and the on-air state stay separate in both modes

**This is the load-bearing half.** In Follow, the cursor moves **locally and
instantly** — it is the operator's own position and nothing needs to confirm it.
The **on-air indicator still waits for the node's ack**, exactly as in Cue mode.

They are two indicators, they are already visually distinct, and merging them is
the one thing this ADR forbids. On a booth LAN the ack is single-digit
milliseconds, so nothing feels slower; what is prevented is a slide being
*declared* on air before it is. Follow mode makes that failure worse than in Cue,
not better: the operator is heads-down on the music and will never see a
correction.

### A newer take supersedes an older one that has not fired

Rapid navigation means cue *n+1* arrives while *n* is still in flight. Queueing
them all makes the layer flash through every slide the operator passed, which is
both wrong and slow.

**A take to a layer supersedes an unfired take to the same layer**, and the
superseded cue is acked as `Superseded` rather than dropped silently. The
operator's intent is their latest position, not the history of how they got
there. This adds `CueResult::Superseded` to `pmg-proto`.

Note what this is not: it is not the deadline rule. A late cue is still
**rejected** (ADR 0007). Superseded means *replaced by a newer intent*, and a
control surface or a log reader must be able to tell the two apart, because one
is the system working and the other is the system refusing.

### Cues pipeline in Follow

`POST /api/cue` stays synchronous per cue and stops being the only path. Follow
sends over the existing socket without blocking the next keypress on the previous
ack, and the UI resolves each ack against the cue id it minted. The synchronous
HTTP endpoint remains the surface for Cue mode, for control surfaces, and for
anything scripted.

### Mixing modes in one rundown

A rundown holds playlists of both kinds — that is the normal Sunday. Moving the
cursor from a Cue playlist into a Follow playlist **changes what the keyboard
does**, so the mode has to be visible without looking for it: a persistent
indicator, not a setting buried in a menu.

## Consequences

- `playlists` gains a `mode` column; the wire and the API carry it.
- The keyboard map becomes a function of the focused playlist's mode. That is a
  real cognitive cost for an operator crossing a boundary mid-service, and the
  indicator is the mitigation rather than a decoration.
- Show renders cursor and on-air as separate states **always**, in both modes.
  Code that merges them is a bug regardless of how it looks.
- `CueResult::Superseded` exists, so the cue log can show a chorus honestly:
  eight requests, five superseded, three on air — rather than either eight takes
  or a mysterious gap.
- Follow's transport is no longer the blocking HTTP endpoint, which means two
  paths to keep behaviourally identical. The dedupe and deadline rules live at
  the **node**, so both paths inherit them; that is why they live there.
- An operator in Follow can outrun the node. The system's answer is to supersede,
  which is correct and means **not every slide they touched appeared on air**.
  That must be true and visible rather than hidden.

## Alternatives rejected

**Mode per Item.** Too granular. An operator's hands cannot change behaviour per
row, and a rundown where some rows fire on click and some do not is a trap.

**Mode per node or per output.** The wrong axis. One node plays the lyrics and
the message in the same service, to the same output, ten minutes apart.

**Mode as an operator preference or a UI toggle.** Rejected as dangerous: two
stations would disagree about whether clicking fires, and the answer would depend
on who logged in last. Mode is a property of the *content* — a song is followed,
a cuelist is cued — so it belongs to the content.

**One mode plus a modifier** — hold a key to take immediately. Rejected: it puts
the mode in the operator's memory rather than on the screen, and it is exactly
wrong for the case where their eyes are on the band.

**Making Follow optimistic** — set on air locally and reconcile later. This is
what the design exploration currently does and it is the one thing rejected
outright. It is the take button lying, in the mode where nobody is watching it.

**Letting Follow queue every cue.** Rejected: the layer flashes through slides
the operator scrolled past, and latency grows the faster they work — the opposite
of what the mode is for.

## Seam

`mode` is an enum on the playlist read with unknown values **preserved**, so a
later third mode — timed auto-advance, a countdown roll — is an addition rather
than a migration. Readers that do not know a mode refuse the playlist loudly
rather than guessing at `cue`, on the same principle as an unreadable template
(`scene-format.md` §3.4).

The cursor/on-air split is what makes any future mode expressible at all: every
mode is a rule about when the cursor's position becomes a cue, and none of them
can be written if the two are one value.

`Superseded` being a distinct result rather than a silent drop is what lets a
control surface (ADR 0014) report honestly on a physical panel with three LEDs,
where "it did not fire because you moved on" and "it did not fire because it was
late" need different lamps.

Related: ADR 0007 (cue ordering, dedupe, deadlines), ADR 0008 (snapshots stream,
cues are acked), ADR 0015 (four modes and plain words), ADR 0018 (one Item,
several destinations).
