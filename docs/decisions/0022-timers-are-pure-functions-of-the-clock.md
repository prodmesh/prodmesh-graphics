# ADR 0022 — Timers are pure functions of the clock, evaluated on the node

Status: accepted · 2026-08-20

## Context

M6's list names "timers and utility data sources — the P1 input types". The
graphic behind that line is the most-used one in the building: **"Service begins
in 4:32"**, counting down on the wall while people find their seats. Close
behind it are a wall clock, a countdown to the end of a countdown-video, and
"how long have we been in this segment" on a stage display.

Every Item today is **static**. Data is bound at arm, layout is solved once, and
a frame after that only evaluates timelines and applies transforms — which is
exactly why a frame costs 3 ms instead of 17. A countdown is the first data that
changes with **nobody touching anything**, and it lands on three questions:

1. **Who computes the value?** A countdown that freezes when the control link
   hiccups is worse than no countdown: it is a *wrong* countdown, on air,
   claiming a number. The autonomy invariant (ADR 0009) already says a node that
   loses the server keeps playing.
2. **What does it cost per frame?** The arm/rebind split is the performance
   contract. Re-solving layout every frame would undo it.
3. **What is it, in the format?** Timers have to be authorable, publishable,
   and refusable by a reader too old to understand them.

## Decision

### A timer is a pure function of the clock

```rust
format_at(spec: &TimerSpec, now_ms: i64, taken_at_ms: i64) -> String
```

No I/O, no state, no failure mode. Same inputs, same string, on every machine
and in the browser — which keeps the VISION rule that *a template must not be
able to throw at air time*, keeps the value testable, and lets a timer appear in
the golden corpus at a **pinned instant** like any other figure.

The P1 kinds, and nothing else:

| Kind | The question it answers |
|---|---|
| `clock` | What time is it? |
| `countdown` | How long until 10:00? |
| `countup` | How long since 10:00? |
| `elapsed` | How long since this Item was taken? |

`countdown` floors at zero and stays there. It does not go negative and it does
not blank: a countdown that reads `0:00` is telling the truth, and one that
vanishes at zero takes the graphic off the wall at the exact moment everyone is
looking at it.

**Formats are enumerated, not a format string.** `hh:mm:ss`, `h:mm:ss`,
`mm:ss`, `m:ss`, `hh:mm`, `h:mm`, and `h:mm am/pm`. A strftime-shaped mini
language is a parser, and a parser is a thing that fails at air time. A fixed
set is total by construction and covers every graphic anyone has asked for.

### The node computes it, from its own clock

The value never travels over the link. The Item carries the **declaration** — a
kind, an instant, a format — and the node evaluates it per frame against its own
clock. A node that has lost the server counts down correctly, which is the whole
point.

The cost of that: **a countdown is only as right as the node's clock.** An
instant is stored as UTC milliseconds, so the arithmetic needs no timezone, but
`clock` shows the node's *local* time and two nodes whose clocks differ show
different numbers. NTP is the mitigation and it is an install requirement, not
an implementation detail. PTP is M6's other question and would tighten this to
sub-frame; nothing here forecloses it.

### The picture changes on the second, not on the frame

Each frame formats the timer and compares the **string**. Nothing happens while
it is unchanged, which is 59 frames out of 60. When it changes, the node does
the rebind it already knows how to do — the `Update` path, "a rebind is an arm,
and it costs what an arm costs" — and that cost was measured at 0.3–2 ms against
a 16.68 ms budget.

**A timer rebind does not morph.** The existing rebind tweens between two solved
layouts, which is right for "the name changed mid-hold" and absurd for a clock:
every tick would start a 400 ms animation and the next tick would interrupt it.
A timer-driven rebind is instantaneous.

### It is a field type, and the Item supplies the timer

The same shape media fields already use (M5o): the **template** says a timer goes
here, the **Item** says which one.

```jsonc
// template
{ "key": "starts_in", "type": "timer", "label": "Countdown" }

// Item data
{ "starts_in": { "timer": { "kind": "countdown", "at": 1755691200000,
                            "format": "m:ss" } } }
```

That keeps one template serving every service, and it keeps the countdown target
where a service's own facts live.

### `FieldType` gets the escape hatch node kinds already have

`NodeKind` preserves an unknown kind and refuses to draw it (§3.4). `FieldType`
had no such escape, so **adding a variant was a hard parse failure** — a
document a 1.3 reader cannot load at all. That is a worse failure than the
format is designed for, and it would have been true of every future field type,
not just this one.

`FieldType::Other(String)` is added in the same version. From 1.4 onward an
unknown field type is preserved, renders as the Item's literal stored value, and
sets `degraded` — the same bargain unknown node kinds get. **1.3 readers still
refuse a 1.4 timer template outright**, loudly, at assignment; that is the last
time a field type does this.

Format **1.4**, `min_reader` unchanged, through the migration chain.

## The alternative, and why it loses

**The server pushes the value.** A tick on the wire, the node renders whatever
arrived. It is less code and it is wrong: it puts a per-second heartbeat on the
control socket, makes the countdown a property of the link's health, and freezes
the number at whatever it was when the link stuttered — a graphic that lies
rather than one that stops. It also breaks the autonomy invariant for the one
kind of graphic most likely to be on screen unattended.

**A format string.** `"%M:%S"` is familiar and it is a parser on the render
thread. The enumerated set is one match statement that cannot fail.

**Re-solving every frame.** Correct, simple, and 17 ms — it spends the entire
frame budget to change a picture that changes once a second.

## Consequences

- A node's clock is now load-bearing. `pmgctl` and the health report should
  surface node clock skew, because "the countdown is 3 seconds out" must have an
  answer that is not guesswork.
- **Tabular figures stop being advice.** Proportional digits change width every
  tick, so a content-sized box breathes and a centred clock jitters — the
  spec has warned about `tnum` since §3.3 and a timer makes it real. The
  compiler warns when a text node bound to a timer field does not ask for it.
- The corpus can hold timer figures, because a pinned `now` makes them as
  deterministic as anything else in it.
- Any surface that shows an Item's data now has a value that is a declaration
  rather than a string, and has to render it as one.

## Seam

What this must not foreclose, and does not:

- **P2 dynamic data sources** — a scoreboard feed, a service-planning API, an
  MQTT topic. Those are values that arrive from OUTSIDE and cannot be a pure
  function of the clock; they need their own ADR, their own staleness story
  ("what does the graphic show when the feed dies?"), and they do not get to
  reuse this one by analogy.
- **Timezones.** `clock` uses the node's local time. A per-destination timezone
  is additive: a field on the screen or the output, not a change to the
  declaration.
- **A timer as an animation driver** — a bar that empties as the countdown runs.
  That is a binding into a *numeric* property rather than text, and the
  declaration already carries everything it would need.
- **Sub-second formats** and frame counts, for a countdown clock on a shot
  clock. Another entry in the enumerated set.
- **PTP.** Nothing here assumes the system clock is the only clock.
