# What the Canyon Hills rundown changed

Written 2026-08-16, against `canyon-hills-service-rundown.md`.

Sixty minutes of a real service, described by the person who runs it. Four
things it confirmed, four it changed, and one it turned from a deferred nicety
into the reason this project exists.

---

## The headline: the differentiating feature is currently `Unsupported`

> *"The previous section cleanly fades to the next section and the box animation
> tween expands/contracts to fit that text cleanly. (This tweening doesn't
> actually happen today, ProPresenter doesn't support it because it doesn't
> support responsive design)"*

This is the sentence the whole flexbox solver exists for. It is the one thing in
sixty minutes explicitly marked as *impossible on the current system*, and it is
what "the authoring stability of Xpression with the simplicity of ProPresenter"
actually cashes out to in a room.

**Today the node refuses it.** `CueOp::Update` returns
`RejectReason::Unsupported`, with an honest comment: rebinding a live instance
needs a relayout, and the engine does not relayout without re-arming.

That refusal was correct when it was written — silently showing stale text is
worse — and it is now the top of the backlog rather than a footnote. The shape of
the answer is already in the architecture:

- Layout is solved **at arm**, and solving is 0.3–2 ms.
- So solve **both** states at arm: the verse as it is, and the verse as it will
  be.
- Then interpolate the two solved rect sets over a duration, and cross-fade the
  glyphs.

That is not "relayout per frame", which the arm/take split forbids for good
measured reasons. It is *two* layouts and a tween between them — which is
exactly the trick that makes it cheap, and exactly what nothing else in this
market does. It needs a name, a cue op, and an ADR.

**It is also the strongest argument yet for the responsive layout work already
done.** The band growing and shrinking to fit the text was verified as numbers in
M1b. This is that same solver, animated.

---

## Confirmed, decisively

**Multi-destination is not an edge case, it is the norm.** Nearly every row of
the rundown lists two or three: `Lower Third Key/Fill` + `Full Screen 1080p` +
`LED Wall`. ADR 0018 was written from one example; the rundown shows it is the
default shape of almost every graphic in the service.

**Follow mode is the worship set.** *"Operator begins pressing Right arrow key to
step thru song, following the music."* Twenty-five minutes of the hour, four
songs, exactly ADR 0017's Follow.

**Jump-to-section is required, not a nicety.** *"Worship leader suddenly decides
to sing the Chorus again. Operator clicks the first slide in the 'Chorus' to go
back."* This is the design exploration's song-structure map (Verse 1 / Chorus 1 /
Bridge) earning its place — the operator needs to jump to a *named section* under
time pressure, not scroll a list looking for it.

**Partial delivery matters more than assumed.** Three destinations per graphic
means three ways for one to be missing, on every take, for an hour.

---

## Changed: two additions to ADR 0018 before it is built

### A destination can be deliberately blank for an Item

> *"Full screen graphics typically blank out the LED wall leaving only a
> background."*

So an Item may name a destination and intend **nothing** to be rendered there.
That is not the same as a destination that failed, and not the same as one that
was never configured. Three states, and the operator interface has to tell them
apart:

| | meaning |
|---|---|
| rendered | this Item is on that feed |
| **deliberately blank** | this Item says "clear that feed" |
| unavailable | that feed did not receive it |

A blank destination is an **instruction**, and it has to travel as one — the LED
wall going to background is part of what "take this graphic" means.

### Not every destination fires on every take

> *"Lower thirds always have a full screen version we can take if we need to, and
> a LED wall version."*

The lower third and the wall go together. The full-screen version is **available
on demand**. So destinations need a default — *automatic* or *on request* — and
the operator needs one action for "take it" and another for "also put it full
screen".

ADR 0018 as written fans out to everything. That is the common case and not the
whole case, and the difference is one field per destination rather than a
redesign.

---

## Absent, not deferred: things with no entry anywhere

**Audio to Dante.** *"Audio: 4 channels, played into Dante Virtual Soundcard"*
for announcements; *"Audio: 2 channels"* for the sermon bumper. Today audio
leaves this system embedded on SDI or inside NDI. Dante Virtual Soundcard is a
**CoreAudio device** — a different backend entirely, not a configuration of the
existing ones. And the channel count is a property of the **clip**, not only of
the output.

`output-config.md` §7 assumes audio rides with video. Half the audio in this
rundown does not.

**Timecode as a clock, not a button.** *"In advanced services, this can be
programmed to LTC or Midi Timecode."* M4 has MIDI as a control *surface* —
something that presses a button. Being *driven* by LTC is a different thing: a
clock source that the rundown follows. Different design, different failure modes.

**The pre-service state, which is on screen longest.** Auto-rotating announcement
slides with a countdown timer that *"auto-clears (slides out) at zero."* Every
part of that is deferred to M6: auto-advance, timers, and an element that removes
itself on a condition. It is also the first thing the room sees and it runs for
longer than anything else in the service.

---

## What this changes about priorities

Nothing here breaks an ADR. Two extend 0018 before it is implemented, which is
the cheapest possible moment to find out.

The reordering it argues for:

1. **The responsive tween.** It is the differentiator, it is currently refused,
   and the architecture is already shaped to allow it.
2. **ADR 0018 with the two additions**, then 0017.
3. **An audio-only output backend**, because a quarter of this rundown's audio
   has nowhere to go.
4. M4's control surfaces, which is where LTC would land alongside MIDI.

The pre-service block stays M6, but it should be understood as *the longest
continuous thing on screen*, not as a utility.
