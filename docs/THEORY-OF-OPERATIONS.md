# Theory of operations

How a graphic actually gets on air. This is the document to hand someone who
asks that question; everything else in `docs/` is detail hanging off this spine.

## The two authorities

Everything below follows from one rule, and it is worth stating before the
mechanics:

> **The node is the authority on what is on air.**
> **The server is the authority on content and assignment.**

A server restart must not change a single pixel. On reconnect the server *reads*
the node's `StateReport` and adopts it; it never pushes its idea of on-air state
back down. If the server's rundown disagrees with what the node reports, the UI
shows the divergence and offers an explicit reconcile — it never silently
re-fires. That rule is what makes node autonomy real rather than aspirational,
and it is why cue authority can safely be distributed to hardware surfaces that
terminate at the node.

## The twelve hops

| # | Hop | Artifact produced | Format | Where it lives |
|---|---|---|---|---|
| 1 | **Author** in Design | **Template Document** — authoring source: layers, guides, notes, sample data, editor state | JSON | server SQLite (`templates`, `template_drafts`) |
| 2 | **Compile** — in-browser WASM, the same code `pmgctl` runs | **SceneDoc** + **manifest** | canonical CBOR + JSON sidecar | server CAS |
| 3 | **Publish** | **Template Revision**, `template_rev = blake3(scenedoc)` | — | `template_revisions`; the `published` pointer moves |
| 4 | **Configure** | **Node / Output / Layer-stack config** | JSON in SQLite | server, mirrored to node |
| 5 | **Edit** | **Item** `{template_id, template_rev, data{}, destination{node,output,layer}}`, **Playlist**, **Rundown** | SQLite rows | server |
| 6 | **Assign** | **Node Assignment** `{gen, outputs[], layers[], template_revs[], items[], asset_hashes[]}` | CBOR | server → node, **persisted on node disk** |
| 7 | **Stage** | asset sync — node pulls every missing hash over HTTP | bytes | node CAS, cached forever |
| 8 | **Arm** | **SceneInstance** in the layer's preview slot | RAM | node |
| 9 | **Take** | **Cue** `{cue_id, seq, epoch, op, deadline_ms}` → **CueAck** `{fired_at_frame, at_utc}` | CBOR over PMG Link | node authoritative |
| 10 | **Render** | RGBA16F linear premultiplied texture, per layer | GPU | node |
| 11 | **Composite** | one output framebuffer | GPU | node |
| 12 | **Output** | fill / key / fill+key / embedded alpha, colour-converted | SDI · NDI · display | hardware |

### 1–3: authoring and publishing

A designer works on a **Template Document** — the editable source, which keeps
things the renderer does not care about: guides, layer names, notes, sample data
for preview, collapsed/expanded state in the tree.

Pressing publish **compiles** it to a **SceneDoc**: the layout box tree, the
binding schema, the timelines, and the asset references, with everything the
editor needed stripped out. The compiler is `pmg-scene` — the same Rust code
compiled to WASM in the browser and to a native binary in `pmgctl`. There is one
compiler, so a template compiled on a laptop and one compiled on a build server
are byte-identical.

The SceneDoc is encoded as **canonical CBOR** and hashed. That hash *is* the
**Template Revision**. Revisions are immutable: editing a template creates a new
one. An Item pins the revision it was authored against, so republishing a
template never changes anything on air — moving an on-air Item to a new revision
is an explicit operator action, with a UI that says what changed.

### 4–5: configuration and content

**Configure** describes the plant: which nodes exist, what outputs each has,
their format and colour space, whether each is fill, key, a fill+key pair, or
embedded alpha, and the **named layer stack** each output carries — typically
Bug, Lower Third, Ticker, Fullscreen, bottom to top. Each layer holds at most one
live graphic. That is what makes "take a new lower third" mean "replace what is
on the Lower Third layer" without anyone naming a layer number.

**Edit** produces **Items**. An Item is data plus a destination: which Template
Revision, the values for its fields, and which node/output/layer it goes to.
Items group into **Playlists**; Playlists compose a **Rundown**.

### 6–7: assignment and staging

The server computes what each node needs — every Template Revision referenced by
every Item destined for it, plus the transitive set of asset hashes — and offers
it as an **Assignment** with a monotonic `gen`.

The node persists the assignment to disk **and only then acks it**. That ordering
is the entire autonomy invariant in one line: anything the node has acked, it can
reconstruct from local disk with no server present.

It then **stages**: fetches every missing asset hash over plain HTTP with Range
and resume, verifies each against its hash, and renames it atomically into its
local CAS. Assets never travel over the control socket — a 4 GB video must not
head-of-line-block a take.

**An Item is not armable until its node reports ready.** Not "warns" — *not
armable*. The Show view greys the button and says which asset is still
transferring. This is the enforcement point that makes "every byte before air"
a property rather than a hope.

### 8–9: arm and take

**Arm** builds a `SceneInstance`: the SceneDoc with the Item's data bound in,
layout solved, text shaped, glyphs rasterized, media opened and pre-rolled. All
the expensive work happens here, off air, so that take is cheap.

**Take** is a **Cue**. Cues are the sharp end of the whole system and get their
own guarantees:

- `seq` is monotonic per `(epoch, node)`; the node holds `last_applied_seq` and
  acks a replay without re-firing it.
- `cue_id` is generated **in the browser at button-press** and carried end to
  end, so an operator's double-click or an HTTP retry after a timeout dedupes at
  the node.
- Every cue carries `deadline_ms`. **A take whose deadline has passed is
  rejected, not fired late.** A graphic that appears four seconds after the
  button because the link hiccuped is worse than one that never appears.
- `POST /api/cue` does not return until the node acks or the deadline expires. On
  expiry it returns `503 cue_not_acked` and the Show view says so in words. **The
  operator's button never lies.**

### 10–12: render and output

The node's layer engine evaluates each live `SceneInstance` at the frame
boundary, renders each to an RGBA16F linear premultiplied texture, composites the
layer stack into the output framebuffer, and hands it to the output backend,
which does the colour conversion and the key generation.

**Colour discipline, because this is where it goes wrong:** composite in linear
premultiplied float and encode exactly once, at output. Fill goes RGB→YCbCr
through the configured matrix. **The key does not** — it is a monotone alpha
signal, written as `Y = alpha` mapped to the configured range with neutral
chroma. Putting the key through a colour matrix makes soft edges subtly wrong
downstream, in a way that is very hard to see on a confidence monitor and very
obvious on a switcher's keyer.

## Timing

**One render thread per output group, pumped by that group's own frame callback**
— the DeckLink completion callback, NDI send pacing, or vsync. Two references
means two render loops. A vsync-driven display output is never in a phase-locked
group, and never claimed to be.

Timeline time is evaluated at frame boundaries from exact rational fps:

```
t_ms = frames_since_start * 1000 * fps_den / fps_num     // 60000/1001, never 59.94f
```

so an 800 ms In looks the same at 25p and at 59.94p.

A cue arriving between frames is scheduled for the first uncommitted frame. With
DeckLink's 3–5 frame preroll that is typically N+3. **The ack carries the frame
number it was actually scheduled for and a UTC estimate**, so the cue log shows
when the graphic *went on*, not when someone asked for it.

### What phase-lock does and does not promise

| Scope | What locks it | Promised in v1? |
|---|---|---|
| Multiple outputs on one DeckLink device | Same scheduled display time on a shared device clock | **Yes** |
| Two DeckLink cards in one machine | Reference to both, from a DA | Yes, with the wiring |
| Across machines | Same blackburst / tri-level to every node | Frame *timing* aligns |
| GPU display output | Nothing, absent a Quadro Sync card — and that locks to the display, not to SDI | **No** |

**Reference gives you phase, not frame numbering.** Two nodes locked to the same
generator emit frames in lockstep but do not agree on which frame is number
1841944. Frame-accurate simultaneous takes across nodes needs timecode from the
same generator or a scheduled-take-at-a-timestamp protocol. v1 does not promise
it. The enabling primitive — the `at:` field on `Cue` — ships from day one
anyway, because it is free now and a protocol break later.

## Snapshots stream, cues are acked

ProdMesh-Control's `server/streamHub.js` conflates frames under backpressure —
at most one pending frame per topic per connection, newest wins — and its own
comments say why that is correct, and where it stops being correct:

> This is correct *because every topic carries a state snapshot* [...] It would
> be wrong for an event-shaped topic ("a cue fired"), where a superseded value is
> a lost event.

A graphics system is mostly event-shaped, so the line has to be drawn explicitly.

**Snapshot-shaped** — SSE topics, conflate freely, newest value is the whole
truth: `node:<id>:health`, `node:<id>:telemetry`, `node:<id>:sync`,
`output:<id>:layers` (layer occupancy), `item:<id>:readiness`,
`rundown:<id>:cursor`, `preview:<id>:stats`.

**Event-shaped** — never on a conflating transport: `take`, `take-out`, `update`,
`clear-layer`, `clear-all`, `timeline-jump`, `playlist-advance`. These travel on
PMG Link with `seq` and acks, idempotent on `(epoch, cue_id)`.

**And the bridge between them, which is the useful part.** Operators want a cue
feed in the browser — "Lower Third · taken 10:15:03.12 · frame 1841944". That is
event-shaped data that a browser wants over SSE. The rule:

> **An event stream reaches the browser as the snapshot of a log tail.**
>
> Topic `show:<id>:cuelog` publishes `{ head_seq, entries[] }` — a sliding window
> of the last N entries. Conflation is safe again, because the newest frame
> *contains* everything the dropped frames carried. A client that has been away
> sends `?since=<seq>` and gets the gap over plain HTTP.
>
> Never publish a bare `{event: "take", item: …}` frame on a conflating topic.

## What happens when things break

**The link dies.** The node keeps rendering. Application heartbeat is 1 s ping /
3 s dead, because a TCP connection over failing wifi can take 30+ seconds to
notice and the UI must grey that node's layers within seconds. Cues that arrive
after a long reconnect are killed by their deadlines rather than firing into a
service that has moved on.

**The server dies.** Everything on air stays on air. The node serves its own
minimal status page and cue API on `:8520`, and hardware surfaces — GPI, MIDI,
OSC — terminate at the node, not at the server, so an operator can still take and
clear. When the server returns, it adopts the node's reported state.

**The node reboots.** It loads `assignment.cbor`, verifies every referenced hash
is present in its CAS, opens its outputs, and comes up **black**. Auto-restoring
a graphic after a power cut is an unpleasant surprise on an unattended machine;
restore is an explicit per-output opt-in.

**A cue is duplicated.** Deduped at the node on `(epoch, cue_id)`.

**A cue is lost across a reconnect.** The node reports `last_applied_seq` in its
`Hello`; the server replays from a 1024-entry ring, or declares a full resync.

**An asset is missing.** The Item is not armable and the Show view says which
asset and which node. Discovered while staging, on Saturday.

**A node is too old for a template.** Rejected at *assignment* time, loudly, with
the version numbers — not at take time. Same principle: fail early and in
daylight.
