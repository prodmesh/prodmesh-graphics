# Spec — PMG Link

**Normative.** The control-plane ↔ node protocol. Decisions and reasoning:
[ADR 0007](../decisions/0007-pmg-link-discovery-assignment-and-cues.md),
[ADR 0009](../decisions/0009-nodes-are-autonomous.md).

| | |
|---|---|
| Transport | WebSocket, binary frames, CBOR payloads |
| Direction | **The node dials the server** |
| Endpoint | `ws://<server>:8519/link` |
| Current `proto_ver` | `1` |
| Heartbeat | 1 s ping / 3 s dead |

## 0. Three connections, and why

| Channel | Transport | Carries |
|---|---|---|
| **Control** | WebSocket `/link` | Handshake, assignment, cues, state, telemetry |
| **Assets** | Plain HTTP `GET /cas/:hash` | Templates, fonts, images, media |
| **Preview** | Separate socket, port 8521 | Encoded preview video |

> **Assets never travel over the control socket.** A 4 GB video must not
> head-of-line-block a take. **Preview never shares the control socket.** A
> preview stall must not delay a cue.

This split *is* the mitigation for most of the cue-delivery risk in R8. It is
structural, not a scheduling heuristic.

## 1. Discovery

The server advertises DNS-SD `_pmg-control._tcp` with TXT records
`ver=<proto_ver>`, `id=<server_id>`, `name=<display name>`.

A node with no stored pairing browses for it and connects as **unadopted**,
appearing in Configure as "New node — Adopt?".

> **mDNS is an aid, never an authority.** Manual host entry MUST work everywhere
> discovery appears. Multicast is genuinely unreliable across VLANs and behind
> switches with poor IGMP snooping, and an operator who can type an address must
> never be blocked by a broken multicast domain.

## 2. Handshake

```jsonc
// → node to server
Hello {
  proto_ver: 1,
  node_id:   "01J8…",        // stable, generated once, stored on disk
  boot_id:   "01J9…",        // NEW EVERY PROCESS START
  name:      "Booth-1",
  version:   "0.1.0",
  assignment_gen: 41,         // what it currently holds, or null
  last_applied_seq: 8812,     // §5.2
  capabilities: { … }         // §2.1
}

// ← server to node
Welcome {
  server_id: "01J7…",
  epoch:     19,              // NEW EVERY SERVER START
  resume_from_seq: 8813,      // or null → full resync
  time_ref: { utc_ms: 1786…, monotonic_ns: … }
}
```

### 2.1 Capabilities

Discovered, never configured. The node reports what the hardware actually said.

```jsonc
{
  "outputs": [
    { "device": "DeckLink 8K Pro (0)", "kind": "decklink", "index": 0,
      "modes": ["1080p5994","1080i5994","2160p5994", …],
      "supports_key_fill": true, "paired_port": 1,
      "has_genlock": true, "genlock_locked": true, "ref_format": "1080i5994",
      "audio_pairs": 8 }
  ],
  "gpu": { "adapter": "Apple M3 Max", "backend": "Metal",
           "max_texture_2d": 16384, "max_buffer": 4294967296 },
  "decoders": ["prores","dnxhr","hap","h264","hevc"],
  "cas": { "bytes": 8123456789, "disk_free": 412000000000 },
  "max_layers": 8,
  "scene_reader_major": 1
}
```

### 2.2 Resume versus resync

| `boot_id` | `epoch` | Result |
|---|---|---|
| Same | Same | **Resume.** Replay cues from `last_applied_seq + 1`. |
| Same | Changed | Server restarted; node kept playing. **Server adopts `StateReport`.** New `epoch`, `seq` restarts, cue idempotency keys are scoped to the new epoch. |
| Changed | Same | Node restarted. Full assignment check; node came up black (§7). |
| Changed | Changed | Both restarted. Full resync. |

## 3. Assignment

```
← AssignmentOffer { gen: 42, digest: "blake3:…" }
→ AssignmentNeed  { gen: 42 }              // or AssignmentOk { gen: 42 }
← Assignment      { gen: 42, outputs[], layers[], templates[], items[], assets[] }
→ AssignmentAck   { gen: 42 }
```

`gen` is monotonic per server `epoch`.

> **`AssignmentAck` is sent only after the assignment is durably written to
> `assignment.cbor`.** That ordering is the entire autonomy invariant in one
> line: anything a node has acked, it can reconstruct with no server present.

The digest lets an already-current node answer `AssignmentOk` without a transfer.

## 4. Staging

The node fetches every asset hash it lacks over HTTP (`GET /cas/:hash`, Range and
resume supported), verifies each against its hash, and renames it atomically into
its local CAS. Concurrency is bounded; a failed fetch retries with backoff and
never blocks the control socket.

```jsonc
→ SyncProgress { total_bytes, done_bytes, pending: 12,
                 current: "blake3:9f2c…", errors: [] }
→ Ready { assignment_gen: 42, items_ready: ["…"], items_blocked: [
            { item: "…", missing: ["blake3:41ab…"] } ] }
```

> **An Item is not armable until its node reports it ready.** Not "warns" — *not
> armable*. The Show view greys the button and names the file and the bytes
> outstanding.

**A node MUST reject a template whose `min_reader` exceeds its own
`scene_reader_major` here, at assignment time**, with both version numbers in the
error — never at take time.

## 5. Cues

```jsonc
// ← server (or a local surface) to the layer engine
Cue {
  cue_id:      "01JA…",     // generated at the ORIGIN — see §5.3
  seq:         8813,
  epoch:       19,
  deadline_ms: 500,          // null for arm/preload
  at:          null,         // §5.5 — reserved, always null in v1
  op:          { … }         // §5.1
}

// → ack
CueAck {
  seq: 8813, cue_id: "01JA…",
  result: "applied" | "deduped" | "rejected",
  reason: null | "deadline_passed" | "not_ready" | "no_such_layer"
        | "unreadable_template" | "no_such_item",
  fired_at_frame: 1841944,
  at_utc: 1786487930986
}
```

### 5.1 Operations

| Op | Payload | Effect |
|---|---|---|
| `Arm` | `{item}` | Build the `SceneInstance` into the layer's preview slot. Nothing appears. |
| `Take` | `{item, layer}` | Move to program, play `In`. |
| `TakeOut` | `{layer}` | Play `Out`, then evict. |
| `Update` | `{layer, data}` | Rebind data on the live instance; play `Update` if present. |
| `ClearLayer` | `{layer}` | Evict immediately, no animation. |
| `ClearAll` | `{output}` | Evict every layer on the output immediately. |
| `Timeline` | `{layer, name, action}` | `action ∈ play \| stop \| goto` |
| `Preload` | `{item}` | Decode and cache; no slot occupied. |
| `Preview` | `{output, source}` | Configure the preview channel. |

### 5.2 Ordering and idempotency

- `seq` is monotonic per `(epoch, node)`.
- The node holds `last_applied_seq`. **A cue with `seq <= last_applied_seq` is
  acked as `deduped` with no effect.**
- A gap triggers `→ CueGap { from, to }`; the server replays from a **1024-entry
  ring**, or sends `Resync` if the gap exceeds it.
- **Be honest about what this is for.** TCP already provides ordered reliable
  delivery. `seq` and acks earn their keep **across reconnects** and for
  idempotency — which is exactly where a naive design loses cues, and exactly
  what nobody tests. Do not add machinery for reordering; TCP solved that.

### 5.3 `cue_id` originates at the surface

**`cue_id` is generated where the human acted** — in the browser at button-press,
in the Companion module at key-down, at the node on a GPI edge — and carried
unchanged end to end.

That is what makes an operator's double-click, a bouncing GPI contact, or an HTTP
retry after a timeout dedupe at the node. **This is the failure nobody designs
for and the one that will actually happen.**

### 5.4 Deadlines

> **A `Take` whose deadline has passed is rejected, not fired late.**

A graphic appearing four seconds after the button because the link hiccuped is
worse than one that never appears. Defaults: 500 ms for `Take`, `TakeOut`,
`Update`, `Clear*`; none for `Arm` and `Preload`.

This is also what kills zombie cues that arrive after a 20-second reconnect.

### 5.5 `at` — reserved

`at: { frame: n } | { timecode: "HH:MM:SS:FF" } | null`.

**Always `null` in v1.** The field exists because adding it later is a protocol
break, and because cross-node scheduled takes will need it
([ADR 0011](../decisions/0011-sync-external-reference-first.md)). A v1 node MUST
reject a non-null `at` with `reason: "unsupported"` rather than ignoring it.

### 5.6 The synchronous edge

`POST /api/cue` on the control server does not return until the node acks or the
deadline expires. On expiry: `503 cue_not_acked`. **The operator's button never
lies.**

## 6. State, telemetry, liveness, time

### 6.1 `StateReport`

Sent on every change and on a 1 s heartbeat.

```jsonc
→ StateReport {
  gen: 42, last_applied_seq: 8813,
  outputs: [ { id: "out-0", layers: [
      { name: "Lower Third",
        occupant: { item_id: "…", template_rev: "blake3:…",
                    phase: "in" | "hold" | "out",
                    timeline: "In", t_ms: 320 } },
      { name: "Bug", occupant: null } ] } ]
}
```

> **The server adopts this. It never overwrites it.** If the server's rundown
> disagrees, the UI shows the divergence and offers an explicit reconcile. It
> never silently re-fires.

### 6.2 Telemetry

1 Hz, snapshot-shaped — the one message that maps straight onto an SSE topic:
`fps`, `frames_dropped`, `late_frames`, `gpu_ms`, `cpu`, `gpu_mem`, `temp`,
`genlock_locked`, `output_underruns`, `cas_bytes`, `disk_free`.

### 6.3 Liveness

**1 s ping / 3 s dead**, at the application layer. A TCP connection over failing
wifi can take 30+ seconds to report an error, and the UI must grey that node's
layers within seconds.

### 6.4 `TimeSync`

Ping-pong for millisecond agreement on cue-log timestamps.

> **`TimeSync` is not a video-phase mechanism.** Video phase comes from genlock
> ([ADR 0011](../decisions/0011-sync-external-reference-first.md)). Someone will
> assume otherwise. It is stated here in bold for that reason.

## 7. Standalone operation

A node with a valid persisted `assignment.cbor` and a complete CAS runs forever
with no server.

On boot with no server:

1. Load `assignment.cbor`.
2. Verify every referenced hash is present and correct.
3. Open outputs per the assignment.
4. **Come up black.** Auto-restore is an explicit per-output opt-in — an
   unattended machine re-firing a graphic after a power cut is a bad surprise.
5. Serve the node API and a minimal status page on `:8520`.
6. Keep dialling the server with backoff, forever.

**Local surfaces terminate here.** GPI, MIDI, OSC, and the `:8520` API construct
the same `Cue` messages and feed them straight into the layer engine
([ADR 0014](../decisions/0014-control-surfaces-and-where-they-terminate.md)).
The node publishes every resulting state change, which is why §6.1's adoption
rule is not optional.

## 8. Errors and reconnect

- Any protocol violation: close with a CBOR `Fatal { code, message }`, log both
  sides, reconnect with backoff (1 s → 30 s, jittered).
- `proto_ver` mismatch: `Fatal { code: "proto_version" }` naming both versions.
  Configure shows it as an actionable message, not a stack trace.
- **A reconnect never changes what is on air.** It reconciles the server's view
  to the node's.
