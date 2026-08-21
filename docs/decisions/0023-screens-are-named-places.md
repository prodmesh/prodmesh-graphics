# ADR 0023 — A screen is a named place, and the node never hears about it

Status: accepted · 2026-08-21

## Context

ADR 0018 gave an Item several destinations. Each one is a triple —
`node_id / output / layer` — plus its own pinned `template_rev`. That was the
right shape for making one Item reach two places, and living with it has shown
what it costs:

- **The wiring is in every Item.** Moving the LED wall from `booth-1` to
  `booth-2`, or renaming its output, means editing every Item that goes there.
  There are hundreds in a year.
- **The treatment is in every Item, too.** "The wall look is the full-screen
  template" is a fact about the wall, and it is currently restated in every
  Item's destination list, where it can be restated *wrongly*.
- **Operators are asked about plumbing.** Building an Item means choosing
  `booth-1 / wall / Graphics`. Nobody in the building calls it that. They call
  it the LED wall.

M6's roadmap line calls this "multi-screen destinations", and half of it landed
with ADR 0018. This is the other half.

## Decision

**A screen is a named place in the building, configured once.**

```jsonc
{ "id": "scr_wall", "name": "LED Wall",
  "node_id": null, "output": "wall", "layer": "Graphics",
  "template_id": "01JCHFULLSCREEN..." }   // the default treatment, optional
```

An Item's destination may name a **screen** instead of a triple. Both forms stay
legal: a one-node install with three outputs should not be forced through an
abstraction before it has anything to abstract, and Configure has to be able to
address hardware directly to prove it works at all.

### The node never hears about it

Screens resolve on the **control server**, at assignment build and at cue
dispatch. The node's contract is unchanged: it knows outputs and layers, and a
cue still names them. No protocol change, no reader-version question, and a node
running today's build works with screens the day they are added.

That boundary is the same one ADR 0004 draws for the scene format. The thing
that renders should know as little as possible about how the room is organised.

### The screen names a TEMPLATE; the Item still pins a REVISION

This is the invariant that decides the whole design. Items pin
`template_rev` so that publishing a new revision never changes what is already
on air (ADR 0005 §9.1). If a screen supplied the revision at take time, every
publish would silently change every Item that goes there — precisely the failure
the pin exists to prevent.

So a screen carries `template_id` — "graphics going to the wall are the
full-screen template" — and the **server resolves it to that template's head
revision when the Item is saved**, writing the pin into the destination row as
it does today. The screen supplies the default; the Item still owns the answer.
Changing a screen's template does not touch a single Item that already exists;
moving them forward is the same guided republish M5v built for revisions.

### Deleting a screen that Items point at is refused, and names them

The convention the fields editor already follows. The alternative — orphaned
destinations that silently render nowhere — is the kind of failure discovered by
a graphic not appearing on a Sunday.

For a row orphaned some other way (direct database surgery, a restore), the
destination keeps the last known triple and resolution falls back to it, so a
graphic goes where it used to go rather than nowhere. That is a backstop, not a
workflow.

## The alternative, and why it loses

**Screens on the node.** Let the node hold the mapping and have cues name a
screen. It moves the room's organisation into the thing whose job is to render,
means every screen change is an assignment round trip, and makes a node that
loses the server unable to be told anything about where graphics go — while
gaining nothing, since the server already has to resolve names for the UI.

**A screen owns the revision, not the template.** Simpler to write and it breaks
the pin: every publish becomes a live change to every Item on that screen. The
whole revision model exists to make "what is on air" answerable and stable.

**Replace the triple entirely.** Forcing every destination through a screen
would mean a fresh install cannot address its own hardware until somebody
invents a name for it. Configure has to work before the abstraction exists.

## Consequences

- The server resolves screens in two places — assignment build and cue dispatch
  — and those two must not disagree. One function, called from both.
- An Item's destination list becomes readable to a person: "Lower Third Key" and
  "LED Wall" rather than two triples.
- A screen's template becoming stale is now a thing that can happen, and it
  reuses the revision UI's existing answer rather than inventing one.
- Configure grows screens, which is the first thing in Configure that is about
  the ROOM rather than about a machine.

## Seam

- **Per-screen treatment beyond a template** — a colour space, a safe-area
  policy, a timezone for `clock` timers (ADR 0022's seam). The screen is where
  those belong, and adding them is a column, not a redesign.
- **Screen groups** — "all the lobby displays" as one target. A screen naming
  several outputs is additive; nothing here assumes one.
- **Screens that are not ours.** The ProPresenter-subsumption question
  (`docs/VISION.md`) is about screen CONTENT, and a named place is the piece of
  it that is useful either way — which is why this lands before that question is
  answered rather than waiting on it.
