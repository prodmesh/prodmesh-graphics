# UX brief — what is fixed, what is open, and what is missing

For a design exploration of the operator interface. The current UI is a working
skeleton, not a design: it exists to prove the system end to end, and most of its
appearance is what fell out of building it rather than a decision anyone made.

This document separates the parts that **have reasons behind them** from the
parts that are simply what got built, so a redesign can move freely without
walking into an invariant.

> The reference the owner has named: a **desktop-like application in the
> browser**, in the manner of Figma — panels, a canvas, an inspector,
> keyboard-first, not a website with forms.

---

## 1. What makes an ambitious interface feasible here

**The renderer already runs in the browser.** `pmg-render` compiles to WASM and
draws on WebGPU at **3.34 ms a frame** (measured, M1e), and it is *the same code
the playout node runs* — a template compiled in the browser produces a
byte-identical `template_rev` to the one `pmgctl` produces (ADR 0001).

So a design can assume a **real canvas showing real output**, not a DOM
approximation that drifts from what goes on air. That is the expensive part, and
it is done.

Live state arrives over **one** SSE connection multiplexed by topic, because a
no-TLS LAN appliance gets no HTTP/2 and the browser's six-connections-per-origin
limit is real. Twelve panels watching twelve things is fine; twelve panels each
opening a socket is not.

---

## 2. Load-bearing — a redesign must preserve these

These are decisions with consequences, most of them recorded as ADRs. They
constrain *what states must be representable*, never how anything looks.

### The take button must never lie

`POST /api/cue` is synchronous: it does not return until the node confirms or the
deadline expires. There is deliberately **no optimistic update**. A design needs
a state for **waiting for the node** and a state for **the node did not
confirm** — the latter rendered as a sentence, not a spinner that gives up.

A button that goes green before the graphic exists is the specific failure this
whole path was built to prevent (ADR 0008).

### The cursor and what is on air are two different things

The rundown cursor is the operator's **intent**. What is on air is the **node's
answer**. They are allowed to disagree — someone takes from the rack, a GPI
fires at the node, a cue is rejected — and the UI must show both rather than
reconcile them. It must never silently re-fire to make them agree
(`THEORY-OF-OPERATIONS.md`).

### Readiness gates arming, it does not warn

An Item whose node has not finished staging its assets is **not armable**. Not
"flagged" — unavailable, with the reason named (ADR 0006).

### Layers

Named, ordered, one occupant each. The order **is** the compositing order.
Addressed by name everywhere, so a rename orphans every Item pointing at the old
one — there is deliberately no rename.

### Show has no route into Design

An operator under time pressure should not be one mis-tap from the template
editor. Whatever the shell becomes, that separation is intentional (ADR 0015).

### Vocabulary is enforceable copy

**Template / Item / Playlist / Rundown / Layer / Take / Take out / Clear.** Plain
words in the UI; broadcast aliases (`take_id`, `Scene`) exist in the API only.
`docs/UI_TEXT.md` is normative, including "name the thing, not the failure" —
*"Booth-1 is offline. Anything already on air is still on air."* beats
*"Connection error."*

---

## 3. Wide open — no attachment, change freely

- All layout, density, typography, colour. Tokens exist (`src/styles/tokens.css`)
  but no design pass has happened.
- The four-modes-in-a-sidebar shell.
- Whether Edit and Design are separate modes at all.
- The rack-and-list metaphor in Show.
- Panels, docking, multiple windows, a command palette — none of it exists and
  nothing resists it.
- **Drag and drop.** Avoided so far for *testability*, not principle: jsdom has
  no layout, so a drag-only path is an untested path. The right answer is drag
  **alongside** keyboard and buttons, not instead of them.

---

## 4. Playout modes per playlist — the biggest open question

The original brief said *"playout controls that vary by playlist"*. That has not
been built, and the owner has since made it concrete with two real modes that
behave almost oppositely.

### Mode A — **Follow** (lyrics)

Two lines of a song per slide; the operator follows the music on the arrow keys.
**Navigating IS taking** — there is no preview/program, no arm, no confirm step.
Clicking or arrowing to a slide puts it on air instantly.

The primitives for this already exist: the rundown cursor and the take are
separate operations (M2b), so Follow is *"cursor movement triggers a take"*
rather than new machinery.

What a design must get right here is **speed and position**: where am I in the
song, what is next, and can I hit it without looking down. This is the mode where
a desktop-like keyboard-first interface earns its keep.

### Mode B — **Cue** (the message)

Media-heavy, and explicitly **arm then take** — a traditional graphics cuelist in
the Ross Xpression manner. The owner's stated objection to ProPresenter is
precisely that it lacks this: *the moment someone clicks, it is out.*

This is what the current Show view already does.

### Why this matters beyond UI

**These are not two skins over one behaviour.** A Playlist needs a mode, the
cursor's meaning changes with it, and the keyboard map changes with it. That is a
data-model decision, not a layout one, and it wants an ADR before it is built.

---

## 5. Known gaps a design will run into

Honest list, so the exploration is not surprised.

### One Item, several destinations — **not modelled**

A lyric slide goes to the **lower-third key** (a DeckLink fill+key pair: fill on
one SDI conductor, key on the other) *and* to an **in-room LED wall** over NDI,
usually into a compositor like Pixera or Resolume — same content, different
treatment, possibly a different template per destination.

Today an Item has exactly one `destination: { output, layer }`. Supporting this
means either an Item with several destinations, or a Playlist that fans one piece
of content out across a set. **Neither exists.** This is the largest modelling
gap on the horizon.

### Lyrics as content — **not modelled**

A song is dozens of slides. Items are hand-built one at a time today, which is
fine for eight lower thirds and absurd for a four-verse hymn. Follow mode implies
a content type that expands into slides, not a rundown of hand-made Items.

### Design mode is not an authoring tool

It renders and publishes correctly, and that is all. There is no canvas
interaction: no selection, no moving a box, no property editing, no creating a
template from scratch. It opens a JSON file from disk. Authoring maturity is M5.

### Multi-output navigation

M3c adds **up to four outputs per node**. "Which output am I looking at" becomes
a real navigation problem that the current single-output UI dodges entirely.

### Control surfaces change what the screen must carry

M4 adds Companion, Stream Deck, GPI, MIDI and OSC. Some of what the screen does
today moves to hardware — and GPI, MIDI and OSC **terminate at the node**, not at
the server, so they keep working when the browser is gone (ADR 0014). A design
should assume the screen is one surface among several rather than the only one.

---

## 6. Where to look in the running app

Seeded demo state (a Sunday 09:00 service: three templates, eight items, three
playlists, one rundown) can be recreated on request.

| Mode | State | Worth judging? |
|---|---|---|
| **Show** | Real. Layers, rack, rundown, take log, live preview | **Yes** — most deliberate, most costly to get wrong |
| **Configure** | Real. Adoption, layer stacks, colour, test patterns | Yes, but it is a setup mode and density may be correct |
| **Edit** | Real but plain. Items, playlists, rundown ordering | Yes |
| **Design** | A renderer with a file picker | **Not yet** — judging it would list things already known missing |
