# Spec — Control surfaces

**Normative.** Decisions and reasoning:
[ADR 0014](../decisions/0014-control-surfaces-and-where-they-terminate.md).

## 1. The rule

> **The documented WebSocket/REST control API is the only real surface. Every
> other surface is a thin adapter over it.**

No adapter gets its own cue semantics, its own state model, or its own path into
the layer engine. If an adapter needs something the API cannot express, **the API
grows** — that is the test for whether the feature was real.

Adapters ship in M4, after the API is proven in M2 and M3. Not one before.

## 2. Where each surface terminates

| Surface | Terminates at | Works with the server down? |
|---|---|---|
| Show view (browser) | Server | No |
| ProdMesh-Control / third-party API | Server | No |
| Companion / Stream Deck | Server, **node fallback** | Yes, if pointed at the node |
| RossTalk / DashBoard / Ultritouch | Server, **node fallback** | Yes, if pointed at the node |
| **GPI** | **Node only** | **Yes** |
| **OSC** | **Node only** | **Yes** |
| **MIDI** | **Node only** | **Yes** |

GPI, OSC, and MIDI are physical or console-local signals from inside the
building. Routing them through a server would make them useless in exactly the
failure they exist to cover.

The node's own API on `:8520` accepts the same cue shapes and serves a minimal
status page — the emergency surface when the server is dead.

**Because cue authority is distributed, the take log records `surface` on every
entry** (`http-api.md` §4.2). Without it the log is confusing the first time
someone uses a GPI.

## 3. Shared vocabulary

Every surface uses the same verbs and the same nouns:

```
take · take_out · update · clear_layer · clear_all · arm · preload · timeline
node · output · layer (BY NAME) · item
```

**No layer numbers. No surface-specific aliases.**

**Every surface generates a `cue_id` at the moment the human acted** and carries
it unchanged. A bouncing GPI contact, a double-tapped Stream Deck key, and a
double-clicked browser button all dedupe at the node by the same mechanism.

## 4. Bitfocus Companion

Lives in `~/Code/bitfocus-companion-modules`, with the other Companion modules —
not vendored here.

**Actions:** `take`, `take_out`, `update`, `clear_layer`, `clear_all`, `arm`,
`playlist_next`, `playlist_previous`, `timeline`.

**Feedbacks** (button colour): layer occupied, item on air, item armed, item not
ready, node offline, reference lost.

**Variables:** `$(pmg:layer_<output>_<layer>)` — the on-air item's label;
`$(pmg:node_<id>_state)`; `$(pmg:rundown_cursor)`.

Configuration takes a server host *and* an optional node host. If the server is
unreachable, the module falls back to the node and says so in its status — the
button keeps working.

## 5. RossTalk

A line-oriented TCP listener on **8524**, on both the server and each node.

```
GPI <n>
CC <slot>
CUSTOM take|<output>|<layer>|<item>
CUSTOM takeout|<output>|<layer>
CUSTOM clear|<output>|<layer>
CUSTOM clearall|<output>
```

`CUSTOM` is the real surface; `GPI`/`CC` map onto configured slots for switchers
that only speak those. A DashBoard or Ultritouch panel (in
`~/Code/ross-dashboard-panels`, built with the `ross-dashboard-grid` skill) is a
RossTalk client plus a status poll.

Each connection gets a `cue_id` per command derived from `(connection, sequence)`
so a retried line does not double-fire.

## 6. GPI

**Node-local.** Contact closure via a supported USB or serial GPI box, or a
DeckLink card's GPI where present.

```jsonc
"gpi": [ { "input": 1, "edge": "rising", "debounce_ms": 20,
           "op": "take", "output": "out-0",
           "layer": "Lower Third", "item": "01JB…" } ]
```

**`debounce_ms` is required and defaults to 20.** A bouncing contact is the
normal case, not the exception, and the `cue_id` derived from
`(input, edge, debounce window)` is what makes a bounce that slips through
harmless.

## 7. OSC

**Node-local**, UDP **8525**.

```
/pmg/take        <output> <layer> <item>
/pmg/takeout     <output> <layer>
/pmg/clear       <output> <layer>
/pmg/clearall    <output>
/pmg/timeline    <output> <layer> <name> <action>
```

Outbound state to a configured address, for consoles that display feedback:
`/pmg/state/<output>/<layer> <item_label|"">`.

UDP has no delivery guarantee, so **each message carries an optional trailing
`cue_id` string**. A sender that retransmits should reuse it. A sender that does
not is accepting best-effort, which is the nature of OSC and is documented rather
than hidden.

## 8. MIDI

**Node-local.** Note-on and CC mapped to operations, same config shape as GPI.
Note-off is ignored by default (a take is an edge, not a hold).

Useful mainly where a console operator already has a surface in front of them and
wants two buttons on it.

## 9. ProdMesh-Control

A plain API client, no special path.

The interesting integration is the reverse of what it does today: ProdMesh-Control
reads ProPresenter and Planning Center state and could arm or fire graphics from
Run of Show. That is an ordinary `POST /api/cue` with a `cue_id` it generates.

Its existing integration convention — `isConfigured(cfg)`, `healthKey(cfg)`,
`defaultPort(source)`, one long-lived `watch(cfg, handlers, signal)` that resolves
on abort and never throws for a mere disconnect — is what a `prodmesh-graphics`
adapter on that side should follow, so it slots into `server/health.js` like
every other integration.

## 10. Ports

| Port | Service |
|---|---|
| 8519 | Control server HTTP + SSE + PMG Link |
| 8520 | Node API + status page |
| 8521 | Preview stream |
| 8524 | RossTalk (server and node) |
| 8525 | OSC (node) |

Family 85xx band. ProdMesh RTA is 8600; ProdMesh Caption is 8518.

## 11. The checklist item that matters

> **Stop the control server. Every hardware surface still fires a take.**

That single line in `docs/TESTING.md` is the proof of
[ADR 0009](../decisions/0009-nodes-are-autonomous.md)'s invariant, and it is the
whole reason GPI, OSC, and MIDI terminate where they do.
