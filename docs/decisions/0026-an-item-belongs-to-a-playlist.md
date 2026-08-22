# ADR 0026 — An Item belongs to one Playlist, and a Playlist may repeat one

Status: accepted · 2026-08-21

## Context

An Item was free-floating. `playlist_items` joined Items to Playlists many to
many, with a primary key of `(playlist_id, item_id)` — so one Item could be in
several playlists, and could appear at most once in any of them.

Both halves of that turn out to be backwards, and the scale this is being
loaded to is what makes it obvious. A hundred songs, thirty slides each: three
thousand Items. The only complete view of them was **All items**, a flat list of
every one, which is not a way to find anything. And the shape of the content is
not "a pool of slides that playlists draw from" — it is "a song, which owns its
slides".

Meanwhile the thing that IS shared has no way to be expressed. A chorus comes
round three times in one song. Those are three places and **one piece of
content**: find a typo in it and all three must change. The old key made that
impossible to say at all.

## Decision

**Ownership goes down. Sharing moves inside.**

### An Item belongs to the Playlist it was written for

`items.playlist_id`, and creating one without a playlist is refused rather than
defaulted. An Item created into nowhere is content that exists and cannot be
reached, which is a state migration 16 had to clean up once already.

Copying an Item into another playlist makes a **second Item**. That is the
honest answer: two songs that both say "Verse 1" are two pieces of content, and
a shared instance across playlists is how a fix to tonight's set silently edits
last week's. Moving one takes its places in the arrangement with it, because
entries left behind point at content the playlist no longer owns.

Deleting a playlist deletes the Items it owns. That follows from ownership and
it is stated before it happens, with the count.

### A Playlist's arrangement may repeat an Item

`playlist_entries` — an entry id, a playlist, an item, a position, a section.
The same Item may appear as many times as the arrangement calls for it, and it
is the same Item, so editing it once changes every place it appears. This is
the ProPresenter behaviour the operator asked for, and it falls out of the model
rather than being a feature bolted beside it.

An entry **keeps its id through a reorder** when the client sends it back.
Minting fresh ids every time somebody dragged a row would move the operator's
cursor with it — the kind of thing only ever noticed mid-service.

### The cursor addresses an ENTRY

This is the part that is easy to miss and impossible to patch later. With
repeats, *"the cursor is on the chorus"* is not an answer: the chorus is at
three places, and finding it by item id finds the first — so advancing past the
second chorus would jump backwards to the first.

So `rundown_cursor` gains `entry_id`, and it is what identifies where the
operator is. `item` is still reported beside it, because every consumer takes by
item and none of them should have to learn a second identifier to fire a cue.
`next`/`previous` stay ITEM ids for the same reason, with `next_entry` /
`previous_entry` beside them for the thing the cursor will land on. Two ids for
two questions: what goes on air, and where the operator now is.

### The library is filtered, not listed

**All items is gone.** In its place:

- **Tags on the Playlist**, free strings, no tag table. The rail filters songs,
  and tagging thirty slides "Christmas" would be thirty edits saying one thing.
  A vocabulary nobody can add to without an admin screen is a vocabulary nobody
  uses, so the set on offer is whatever has been typed.
- **A name filter** over the rail.
- **A search** over every Item — its label and the values a person typed into
  it — naming the playlist each result lives in. This answers *"which song has
  that line?"*, which is the question the flat list was being scrolled for. The
  field KEYS are deliberately not searched: matching "title" against every Item
  that has a title field buries the one whose title says what was asked for.

## The alternatives, and why they lose

**Keep the many-to-many and add repeats.** Both kinds of sharing at once. It
loses because the two are not the same relationship and conflating them makes
"edit this Item" ambiguous in exactly the case that matters: the operator cannot
tell, from the row in front of them, whether a fix is local or reaches another
service.

**Tags on the Item, filtering playlists by what they contain.** Asked for
first, and it inverts the work: thirty slides tagged individually to express one
fact about a song, and a rail filter that reads through a join to answer a
question about the parent. Item tags are additive later and are not built.

**A separate "arrangements" table**, ProPresenter's named alternative orders.
Genuinely useful and deliberately not built: this repo has one arrangement per
playlist and no operating experience with a second. The entry table is the seam
— an `arrangement_id` column and a default row is the whole change.

**Leave All items and page it.** The view that does not hold up at this scale,
with a workaround for the symptom.

## Consequences

- Migration 16 rebuilds the join, gives every Item a home, duplicates any Item
  that was in several playlists (which IS the new semantics), files the
  homeless into an "Unfiled" playlist rather than deleting somebody's content
  to satisfy a schema, and translates every cursor to an entry.
- Show keys its rundown rail and slide grid by entry. Keyed by item, a repeated
  chorus collapsed to one running number and lit the wrong row.
- `POST /api/items` requires `playlist_id`. Integrators building Items get a
  422 they did not before, which is the right time to find out.
- A copy loses its `take_id`. The alias is unique and integrator-visible; a
  copy is a different Item and must not answer to the original's name.

## Seam

- **Named arrangements** — "Acoustic", "Full band" — as above: an
  `arrangement_id` on the entry and a default row.
- **Item-level tags**, if sorting inside a large playlist ever needs them. A
  second table, no change to this one.
- **Folders over playlists**, if a hundred songs plus a hundred sermon decks
  makes one flat rail too long even filtered. Tags may well be enough; this is
  the thing to try second, not first.
- **Search across services** — the same endpoint with a rundown filter, once
  there are enough rundowns for "which week did we use that?" to be a question.
