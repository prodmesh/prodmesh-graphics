# ADR 0014 — Control surfaces, and where each one terminates

Status: accepted · 2026-08-14

## Context

Graphics get fired from more places than a browser. The list that matters here:
the Show view, a Bitfocus Companion / Stream Deck surface, a Ross DashBoard or
Ultritouch panel over RossTalk, a switcher's GPO on a GPI input, OSC or MIDI from
a lighting or audio console, and another system's API call — ProdMesh-Control
automating a graphic from Run of Show state.

That is six surfaces, and the obvious plan is to build them. It is also the plan
that produces six half-tested paths into a live show, each with its own idea of
what "take" means, each discovering its own bugs on a different Sunday.

There is a second question underneath, and ADR 0009 forces it. If a node keeps
playing when the server is down, which of these surfaces still works? A Stream
Deck that talks to the server is dead in exactly the failure it exists to cover.
A GPI wired to the node is not. Where a surface *terminates* is therefore not an
implementation detail — it determines whether it works on the day it matters.

## Decision

### One real surface, adapters on top

**The documented WebSocket/REST control API is the only real surface. Everything
else is a thin adapter over it.**

No adapter gets its own cue semantics, its own state model, or its own path into
the layer engine. If an adapter needs something the API cannot express, the API
grows and every surface benefits — that is the test for whether the feature was
real.

Consequences of taking that seriously: adapters ship in M4, *after* the API is
proven in M2 and M3. Not one before. And adapters live where they belong —
the Companion module in `~/Code/bitfocus-companion-modules`, the DashBoard panel
in `~/Code/ross-dashboard-panels` — not vendored into this repo.

### Where each surface terminates

| Surface | Terminates at | Works with the server down? |
|---|---|---|
| Show view (browser) | Server | No — it is a browser talking to a server |
| ProdMesh-Control / third-party API | Server | No |
| Companion / Stream Deck | Server, **node fallback** | Yes, if configured with the node |
| RossTalk / DashBoard | Server, **node fallback** | Yes, if configured with the node |
| **GPI** | **Node only** | **Yes** |
| **OSC** | **Node only** | **Yes** |
| **MIDI** | **Node only** | **Yes** |

**GPI, OSC, and MIDI terminate at the node**, speaking the same cue message
shapes directly into its layer engine on `:8520`. They are physical or
console-local signals from inside the building; routing them through a server
would make them useless in exactly the failure they cover.

Network surfaces prefer the server — it knows the rundown, the playlists, and
what is armed — but Companion and RossTalk can be pointed at a node directly as a
documented fallback, and the node's `:8520` API is the emergency surface with its
own minimal status page.

**The consequence, which is ADR 0009's rule again:** the node publishes every
state change it makes, and the server adopts it. Cue authority is genuinely
distributed and the take log records *which surface* fired each cue — otherwise
the log is confusing the first time someone uses a GPI.

### Shared vocabulary

Every surface uses the same verbs — `take`, `take_out`, `update`, `clear_layer`,
`clear_all` — and addresses the same nouns: node, output, **layer by name**, item.
No layer numbers, no surface-specific aliases (ADR 0010, ADR 0015).

Every surface generates a `cue_id` and carries it end to end, so a bouncing GPI
contact or a double-tapped Stream Deck key dedupes at the node exactly as a
double-clicked browser button does.

## Consequences

- One cue path is tested hard instead of six tested lightly.
- A new surface is a weekend, not a project, because the hard part is done.
- The API is forced to be complete and honest, because it is the only way in.
- **"Stop the server; every hardware surface still fires a take" is a checklist
  item** in `docs/TESTING.md`, and it is the proof of ADR 0009's invariant.
- Adapters are late. Someone will want a Stream Deck in M2 and the answer is that
  the API is not finished yet, which will be unpopular and is correct.
- Adapters living in sibling repos means version coordination across
  repositories — accepted, because a Companion module belongs with the other
  Companion modules.
- Two entry points (server and node) for network surfaces means two places to
  document and two auth stories (ADR 0013's trusted stations covers the node's).

## Alternatives rejected

**Build all surfaces in parallel, early.** What the original scope implied.
Rejected: five half-tested paths into a live show, and no forcing function to
make the API complete.

**Route every surface through the server.** One entry point, one auth model, much
simpler. Rejected because it makes GPI, OSC, and MIDI fail in precisely the
scenario they exist for.

**Let each surface talk to the layer engine with its own semantics.** Fastest per
surface. Rejected: it is how a system ends up with six subtly different meanings
of "clear."

**Vendor the Companion module and DashBoard panel into this repo.** Rejected —
they belong with their siblings, where their own tooling and conventions live.

## Seam

The control API is the product surface. It is OpenAPI-documented, versioned, and
carries broadcast-standard aliases (`take_id` on items, `Scene` as a documented
synonym for Template) precisely so an integrator who has never seen this system
can drive it (ADR 0015).

Because adapters are thin and external, a customer can write their own against
the same document, and the node's `:8520` API is the seam for embedding a node in
a system that is not ours at all.
