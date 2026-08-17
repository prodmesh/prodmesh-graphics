# Dress rehearsal — findings

The Canyon Hills service, built as Templates, Items, Playlists and a Rundown and
run against a node with three outputs. `tools/rehearsal.sh`.

Started 2026-08-17. **In progress** — everything below is from standing the
service up and firing takes from a script. The part that matters, a person
running it at speed, has not happened yet.

Real content: `SONG King Above It All.probundle` and `Message.probundle`, read
with `tools/probundle-read.py`. 41 Items across 4 playlists, 3 destinations on
most of them.

---

## Confirmed working, on real content

**The three destination behaviours, end to end** (ADR 0018). Taking the call-to-
worship verse and then the giving slide leaves:

```
lt    Graphics  Call to worship verse
fs    Graphics  Giving — full screen
wall  Graphics  — clear —
```

`auto` fired, `on_request` did **not** fire with the take, and `blank` *cleared*
the wall the verse had put there rather than leaving it stale. That last one is
the rundown's *"full screen graphics typically blank out the LED wall"* working
as an instruction rather than an absence.

**Per-destination readiness** reports all three destinations of a lyric slide,
aggregated across the node, with `partial: false`.

**Three outputs at 59.94, zero late frames, zero warnings** through the whole
build and a series of takes.

---

## Found: text does not wrap, and never has

**The finding of the rehearsal, and it is not small.** `pmg-text`'s `measure()`
returns a single line height in both of its branches, and the renderer calls
`shape()` once for the whole string. There is no code path anywhere that
produces a second line. `max_lines` is declared in the scene format, carried
through the document, and **ignored**. A hard `\n` in the data is shaped as
part of the run rather than breaking it.

It is written down — `crates/pmg-text/src/lib.rs:373`, *"M1c does not wrap: a
single line, clipped by the box. Real line breaking arrives with parley in
M5"* — and the reasoning was sound at the time: an overflowing template is a
design-time problem an editor can show.

What the rehearsal changes is **when** it is needed. The reference frames from
the real broadcast are:

| Graphic | Lines |
|---|---|
| Lyric slide | 2, with a deliberate break the worship team chose |
| Name tag | 2, but they are two separate fields — this one is fine |
| Scripture chyron | 3–4 of wrapped body text |

Two of the three primary graphics of the service cannot be drawn. Song lyrics
are the single most common thing on screen across a Sunday, and a lyric slide
is two lines by definition. **Line breaking is not authoring maturity, it is
the first content type.** M5 is the wrong milestone for it.

Two distinct pieces of work hide behind one heading, and the cheaper one buys
most of the value:

1. **Honour explicit breaks.** A lyric slide's line breaks are *chosen* — "WE
   ARE HERE TO WITNESS / TO SHARE AND TESTIFY" is a decision about phrasing,
   not an artefact of the box width. Splitting on `\n` and stacking the runs
   needs no line-breaking algorithm at all.
2. **Wrap to a width.** The scripture body block genuinely needs it, and it is
   what makes the box grow and shrink against the words — the 29:30 tween.
   This is the parley-shaped job.

### Correction

Earlier in this same session I reported the rehearsal templates "solve sensibly
with wrapping" and read line counts out of the solved box heights — *"name
h = 101, 278, 343 = 2, 3, 3 lines"*. That was wrong. Those heights are one
line each, of differing natural line heights, and I inferred wrapping from a
number instead of checking whether the code could do it.

---

## Found: the assignment named an output the node does not have

**Fixed.** `assignment.build()` falls back to a single output called `out-0` for
a node with no rows in `outputs`, so that a node is usable the moment it
connects without a trip to Configure. That default is correct for a node running
the default output and wrong for any node started with `--output id=lt,…`.

What it looked like: the node linked, reported healthy, was adopted, accepted an
assignment — and held nothing. One line in its own log said why:

```
WARN the assignment names an output this node does not have; ignoring it output=out-0
```

Nothing on the server or in the browser said anything at all. Every earlier test
used the default single output, which is exactly why this survived to here.

The server now seeds `outputs` from the node's `StateReport` when nobody has
configured any — the node is the authority on what it is driving (ADR 0009), so
taking its word beats inventing a name. Configure still wins once an operator
has been there; re-seeding on every reconnect would undo their layer order on a
reboot.

---

## Found: a field's `max_len` is checked across every destination

**Not fixed — needs a decision.** The reference template declares
`name` with `max_len: 40`, which is right for "Pastor Dan Whitfield" and refuses
every real lyric and every verse. That is the validation doing its job and the
fixture being a fixture; the rehearsal templates now declare their own lengths.

The finding underneath is real, though. `validateDestinations()` checks an
Item's data against **every** destination's template, so **the narrowest
destination binds**. A lower third that caps a line at 160 characters and an LED
wall that allows 400 give the Item 160 — the operator cannot type a line that
the wall could hold and the strip would truncate.

That may well be right: one Item, one set of words, and the tightest constraint
is the real one. But it is currently an emergent consequence of where the check
sits rather than a decision anybody made, and the error names a destination the
operator may not have been thinking about. ADR 0018 says destinations pin
different templates *precisely so they can differ*; this is the one axis where
they cannot.

---

## Confirmed gap: `on_request` has no action

Modelled end to end — stored, assigned, and correctly excluded from the fan-out
at the node — and **there is no operator action that fires one**. The rundown's
*"lower thirds always have a full screen version we can take if we need to"* is
a destination that can only ever be skipped.

Needs one cue op and one button. Small, well-defined, and it is the last piece
of ADR 0018 that the model describes and the system cannot do.

---

## Still untested — the reason the rehearsal exists

These need a person, not a script:

- **Follow at the real pace.** Corrected by the operator: an arrow is pressed
  once at the end of each line, not held down. That is roughly one cue every
  5–15 seconds, and **ADR 0017's stated premise — "an operator holding an
  arrow key through a chorus would queue blocking HTTP requests behind each
  other" — is wrong.** Superseding may still be right for the *jump* case,
  where scrubbing to find a section fires several takes quickly, but the ADR
  currently justifies itself with a rate that does not occur. The ADR needs
  amending rather than the code needing reverting.
- **Jump to a named section.** The reference rundown calls this required. The
  song's real structure — Verse 1, Chorus 1, Verse 2, Bridge 1, Chorus 2, Tag 1,
  Ending — is in the import and goes nowhere: sections survive only as a prefix
  in the Item label. *"The worship leader suddenly decides to sing the Chorus
  again"* currently means reading 16 labels.
- **The tween on real scripture**, in the sermon playlist, at 29:30.
- **Whether 41 Items in 4 playlists is legible at all** in Show.

---

## Note on the .probundle reader

`tools/probundle-read.py` is a reader, never a writer, and deliberately not a
full parser — see its docstring. One thing it surfaced is worth keeping:

**A song's arrangement holds more slots than the document has slides** — 31
against 19 in "King Above It All" — because a chorus is *referenced* by each
section that sings it rather than copied. That is ADR 0018's argument arrived at
from the other direction, in a competitor's file format: content is addressed,
and the places it appears are separate from the content itself.
