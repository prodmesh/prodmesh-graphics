# ADR 0027 — An arrangement names groups, not slides

Status: accepted · 2026-08-21

## Context

ADR 0026 gave a Playlist ownership of its Items and let its order repeat one.
That covers a chorus that comes round three times in the song as written. It
does not cover the other thing a band does: **singing the same song in a
different order on a different Sunday** — the 09:00 short version, the 11:00
full one.

The seam was named in ADR 0026 and this is it. The operator's framing, which is
ProPresenter's and is the right one:

> The current order is "Main", with group names. You can then create derived
> orders, but in those you cannot edit individual items — just the order and
> frequency of the groups. If you want to edit items, go back to Main.

That constraint is the whole design, not a limitation of it.

## Decision

### Main owns the slides. An arrangement owns an order of GROUP NAMES

`arrangements(id, playlist_id, name)` and `arrangement_groups(arrangement_id,
position, section)`. That is all an arrangement holds: a list of names, in
order, repeats allowed.

**An arrangement that stored slides would be a second copy of the song, and the
first typo would prove it** — fixing the chorus in Main would leave every
arrangement still singing the old words. Naming groups instead means an
arrangement follows Main by construction: add a line to the chorus and every
arrangement using it grows one. It is the same reasoning as ADR 0026's repeats,
one level up.

It is also why the operator's constraint is not a restriction we impose. There
is nothing in an arrangement TO edit. The slides are Main's.

### A group is the first consecutive run of a section name

`section` already existed on an entry and is already used exactly this way —
`Verse 1` spanning four slides, `Chorus 1` spanning four. So a group needs no
new column: it is the first consecutive run of entries sharing a section name in
Main.

**The first run, not every entry with that name.** A chorus sung twice in Main
is the same slides appearing twice (ADR 0026), and folding both runs into one
group would make every arrangement sing an eight-slide chorus.

Entries with no section belong to no group and cannot appear in an arrangement.
That is stated in the browser with the slides named, rather than dropping them
quietly.

### Expansion mints an id per occurrence

An arrangement expands to a running order at read time. Each position gets
`<arrangement>:<occurrence>:<main entry>` — deterministic, so the cursor
survives a reload; distinct per occurrence, so it survives a repeat. This is
ADR 0026's reason for entries existing, applied one level up: the cursor has to
be able to tell the second chorus from the first.

### The RUNDOWN pins the arrangement

`rundown_playlists.arrangement_id`, null meaning Main. The choice belongs to the
service, not to the song: 09:00 short and 11:00 full is one song and two
running orders, and putting the choice on the playlist would make the two
services fight over one field.

An arrangement pinned against a playlist it does not belong to is dropped rather
than stored — a rundown pointing at another song's order would expand to nothing
on the morning it mattered. Deleting an arrangement sets the pin back to Main
rather than breaking the running order.

### A song with no named groups is told, not offered

No arrangement control at all, and one line saying that naming the groups in
Main is what unlocks it. A sermon deck of nineteen unnamed slides would get a
picker that does nothing; a button that disappoints is worse than a sentence
that explains.

## The alternatives, and why they lose

**Arrangements hold slides.** The obvious model, and it breaks the moment
anybody fixes a typo: Main and its arrangements are separate copies that drift
silently. Everything good here comes from refusing this.

**Arrangements hold entry ids** rather than group names. Follows edits to a
slide's CONTENT, and not to the song's structure: add a line to the chorus in
Main and every arrangement keeps singing the old two.

**The playlist carries its current arrangement.** Simpler, one place to look,
and it cannot express two services wanting different orders of one song — which
is the case the feature exists for.

**Groups as their own table**, with slides assigned to a group explicitly. More
precise, and it duplicates what `section` already says. It is also a migration
of existing content into a concept it does not have yet; the run-of-sections
reading works on the data as it stands today.

## Consequences

- `getPlaylist(id, arrangement)` reads a song under one of its orders. Absent is
  Main, and Main is the only reading whose rows can be edited.
- Show is unchanged. It reads a running order; where that order came from is
  the server's business.
- Editing Main's sections changes what groups exist. A group an arrangement
  names and Main no longer has is refused at the point of edit rather than
  expanding to a silent gap — but renaming a section in Main *can* orphan an
  arrangement's reference, which shows as a group with zero slides.
- An arrangement starts as Main's groups in Main's order. An empty one is a song
  that plays nothing, and nobody means that.

## Seam

- **Reordering by drag** rather than by up/down buttons. The data does not care.
- **An arrangement per Item destination** — the wall gets the full version, the
  key feed the short one. There is no evidence anybody wants this and it would
  need the cue path to carry an arrangement; noted so it is not accidentally
  foreclosed.
- **Renaming a section in Main** could offer to update the arrangements that
  name it, the way Design offers to move pinned Items forward. Not built: the
  zero-slide group is visible, and a rename is rare.
- **Groups as a first-class thing** — a name, a colour, a default treatment —
  if sections ever need to be more than a label. The run-of-sections reading is
  the compatibility story if that day comes.
