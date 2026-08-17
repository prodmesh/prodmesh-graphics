# ADR 0018 — One Item, several destinations, and what happens when one of them is down

Status: accepted · 2026-08-16

## Context

An Item today has exactly one `destination: { output, layer }`. That was right
for M1 and it does not survive the room it is being built for.

The case that breaks it is the ordinary one. A lyric slide goes to **two places
at once**:

- the **lower-third key** — a DeckLink fill+key pair, fill on one SDI conductor
  and key on the other, into the switcher;
- the **in-room LED wall** — over NDI, usually into a compositor such as Pixera
  or Resolume.

Same words. Different treatment. The key feed wants a narrow strip with a
transparent surround; the wall wants full-bleed type on a background. **Often a
different template**, always a different output, sometimes a different node.

Working around this by making two Items is worse than it sounds. The operator
takes one and forgets the other. They fix a typo in one and not the other. The
rundown grows two rows that must stay in lockstep by discipline, which is the
kind of coupling that holds until the Sunday it does not.

There is a second, sharper question hiding behind the first. **What should happen
when one destination is unreachable?** The wall node is down, the key feed is
fine. Refusing the take because half of it cannot be delivered would put nothing
on air and be defensible in a design document and indefensible in a service.

Settling this before M4 matters concretely: control surfaces are thin adapters
over this model, and five adapters written against one destination get rewritten.

## Decision

**An Item is data plus a list of destinations.** The template moves out of the
Item's identity and into its routing:

```
Item = { fields }  +  [ { node, output, layer, template_rev } ... ]
```

The **data is the invariant** — the name, the title, the two lines of the verse.
Each destination says how that data is rendered and where it lands. One
destination is the common case and stays a one-line answer; the list is what
makes two possible without inventing a parallel Item.

Each destination pins its own `template_rev`, for the same reason a single Item
pinned one: a Thursday edit must not change Sunday's rundown under an operator
who never touched it.

### Item data must satisfy every destination's template

Checked when the Item is created or edited, not at take. Two templates that both
render the same Item must both accept its fields; a destination whose template
requires a field the Item does not carry is rejected **at the point of the
mistake**, with the field named.

### A partly-deliverable take fires what it can, and says what it did not

This is the ruling that matters most.

**A take fans out to every destination and fires the ones that can be fired.** A
destination whose node is unreachable, or which has not staged its assets, does
not block the others. The service continues on the feed that works.

**It is never silent about it.** One `cue_id` fans to N nodes and collects N
results. `POST /api/cue` returns when every reachable destination has acked or
its deadline has passed, and the response carries **a result per destination**.
The button goes green only when all of them fired. Otherwise it says which one
did not and why, as a sentence — *"On air on Booth-1. The wall feed did not
confirm: Booth-2 is offline."*

That is the same rule as ADR 0008, applied to a take that is now plural: the
operator is told exactly what is on air, and partial is a state the interface can
express rather than an edge case it rounds to success or failure.

### Readiness is per destination

An Item is armable when **at least one** destination is ready, and the interface
shows which ones are not and why. This deliberately relaxes ADR 0006's
all-or-nothing readiness, because with several destinations the alternative is
one un-staged asset on a secondary feed making a graphic un-armable everywhere.

## Consequences

- The `items` schema changes: one destination row per Item destination, each with
  its own `template_rev`. The single-destination Item becomes a list of one, and
  existing rows migrate cleanly.
- `Assignment` gains destinations per item, and a node receives only the
  destinations that name it. A node still cannot be told about an output it does
  not have (`seal_outputs`).
- `CueAck` is no longer one answer to one cue. The server aggregates per
  destination, so the cue log records one operator action and several outcomes.
- The take button now has a **partial** state, which is a new thing to design and
  the honest cost of this decision. It is also the state most worth designing:
  it is what an operator meets when a node dies mid-service.
- Editing an Item edits data once and both destinations follow. That is the whole
  point, and it makes "which template does this Item use" a question with more
  than one answer — Edit has to show routing, not a single template name.
- Validation gets stricter: adding a destination can now fail because of the
  Item's existing data. Better there than at 10:15 on Sunday.

## Alternatives rejected

**Leave it at one destination; make two Items.** Rejected: the two rows must be
kept identical by discipline, and the failure is silent and permanent. It is also
the workaround an operator would build themselves, which is the signal it is a
missing feature rather than a preference.

**Fan out at the Playlist rather than the Item.** Attractive — a playlist "goes
to the key feed and the wall" and every item inherits it. Rejected as the
primary model because it cannot express a per-item exception, and there always
is one: the sermon slide that belongs on the wall but not over camera. Kept as a
future convenience that *sets* item destinations rather than replacing them.

**A named Destination Group referenced by Items.** The right shape eventually,
and rejected for now as premature: with one venue and one node there is nothing
to name yet, and a level of indirection that resolves to a single value teaches
nobody anything. The list on the Item is a strict subset of what a group would
expand to, so this stays open.

**Refuse the take unless every destination can be served.** Rejected. It is the
tidy answer and it puts nothing on air when the key feed was fine. The operator's
need is the graphic on the feed that works, plus the truth about the one that
does not.

**Fire what you can and say nothing.** Rejected more strongly than the above:
this is the failure where the wall silently stops matching the broadcast for the
rest of the service, and nobody finds out until someone in the room mentions it
afterwards.

**Per-destination data overrides** — a different title on the wall. Rejected as
scope: it re-opens "which one is the real Item" and there is no request for it.
The binding language's `visible_if` and per-template layout already let two
templates present the same data very differently, which covers the real cases.

## Seam

The destination list is what makes a **Destination Group** an addition rather
than a redesign: a group resolves to exactly this list, so introducing one later
changes how destinations are chosen and not what they are.

Per-destination `template_rev` is also the seam for multi-venue — the same Item,
the same words, a different template per campus — without the Item knowing
anything about venues.

`CueAck` becoming plural is the seam for cross-node simultaneous takes. Once one
operator action already means several acks aggregated against one `cue_id`, the
`at:` field from `pmg-link.md` §5.5 has somewhere to attach, and ADR 0011's
deferred promise about frame-accurate cross-node takes has a shape to land in.

Related: ADR 0006 (assets staged before armable), ADR 0007 (PMG Link, cues and
acks), ADR 0008 (the take button never lies), ADR 0010 (outputs and named
layers), ADR 0017 (playout mode per playlist).
