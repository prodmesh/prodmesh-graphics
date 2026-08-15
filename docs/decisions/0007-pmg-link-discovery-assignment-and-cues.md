# ADR 0007 — PMG Link: discovery, assignment, and ordered acked cues

Status: accepted · 2026-08-14

## Context

The control server and the playout nodes need a channel. What travels on it is
not one kind of thing: node capabilities and health (chatty, small, and safe to
drop), assignments (large, rare, and must be durable), assets (enormous), cues
(tiny, urgent, and must not be lost or duplicated), and preview video
(continuous, and the least important of all).

Treating those as one stream is the mistake to avoid. A design where a cue can
end up queued behind a 4 GB video transfer will work perfectly in testing and
fail on the one Sunday somebody adds a new video an hour before the service.

Discovery is a separate question with its own trap. The family has nothing like
it today — every satellite app is configured with a host and port by hand. But
"type the IP address of your playout node" is precisely the ProPresenter-simplicity
failure this project exists to avoid. Equally, mDNS is genuinely flaky across
VLANs and behind switches that do IGMP snooping badly, and a volunteer who *can*
type an IP must never be blocked by a broken multicast domain.

Full message-by-message detail is in `docs/specs/pmg-link.md`. This records the
shape and why.

## Decision

**Three connections, deliberately separated.** Control on a WebSocket, assets on
plain HTTP, preview on its own socket.

### Transport and direction

**The node dials the server. One WebSocket. CBOR binary frames.**

Node-initiated because reconnect and backoff logic then lives in exactly one
place; it traverses whatever the network does to it; and it matches the mental
model that makes ADR 0009 work — a node is an autonomous appliance that *reports
in*, not a thing that is *driven*.

- Not JSON: binary payloads, timing precision, and one `pmg-proto` definition
  serving both ends.
- Not gRPC: a protobuf toolchain for both a Rust node and a plain-ESM Express
  server, in a family with no protobuf anywhere.
- Not raw TCP or QUIC: WebSocket gives framing, keepalive, proxy tolerance, and
  lets a browser speak it directly for diagnostics.

**Assets go over plain HTTP** (ADR 0006) and **preview gets its own connection**,
so a preview stall can never delay a cue.

### Discovery

**mDNS/DNS-SD as an aid, never as an authority.** The server advertises
`_pmg-control._tcp`. A node with no pairing browses for it and connects as
unadopted, appearing in Configure as "New node — Adopt?". **Manual host entry
always works**, on every screen where discovery appears.

### Assignment

```
← AssignmentOffer { gen, digest }
→ AssignmentNeed  { gen }  |  AssignmentOk { gen }
← Assignment      { gen, outputs[], layers[], templates[], items[], assets[] }
→ AssignmentAck   { gen }        // sent only AFTER persisting to disk
```

`gen` is monotonic per server epoch. **The node persists before it acks.** That
ordering is the entire autonomy invariant in one line.

### Cues

```
← Cue    { cue_id, seq, epoch, deadline_ms, at, op }
           op ∈ Arm | Take | TakeOut | Update | ClearLayer | ClearAll
                | Timeline{name,action} | Preload | Preview{…}
→ CueAck { seq, cue_id, result: applied|deduped|rejected(reason),
           fired_at_frame, at_utc }
```

- `seq` is monotonic per `(epoch, node)`. The node holds `last_applied_seq`;
  anything at or below it is acked without effect. A gap triggers `CueGap` and
  the server replays from a 1024-entry ring.
- **Be honest about what this machinery is for.** TCP already gives ordered
  reliable delivery, so `seq` and acks earn their keep across *reconnects* and
  for idempotency — which is exactly when a naive design loses cues, and exactly
  what nobody tests.
- **Deadline rejection.** A `Take` whose deadline has passed is **rejected, not
  fired late**. A graphic appearing four seconds after the button because the
  link hiccuped is worse than one that never appears. Default 500 ms for takes;
  none for arm and preload.
- **`cue_id` is generated in the browser at button-press** and carried end to
  end, so an operator's double-click or an HTTP retry after a timeout dedupes at
  the node. This is the failure nobody designs for and the one that will actually
  happen.
- **`at: <frame|timecode>` ships from day one** even though v1 always sends "next
  available". It is free now and a protocol break later (ADR 0011).

### State, telemetry, liveness, time

`StateReport` on every change and on a 1 s heartbeat, carrying each output's layer
occupancy. **The server adopts it and never overwrites it** (ADR 0009).

Telemetry at 1 Hz, snapshot-shaped — the one message type that maps straight onto
an SSE topic.

**Liveness is 1 s ping / 3 s dead**, because a TCP connection over dying wifi can
take 30+ seconds to notice and the UI must grey that node's layers within
seconds. Deadline rejection then prevents zombie cues arriving after a long
reconnect.

`TimeSync` ping-pong gives millisecond agreement for cue-log timestamps. **It is
explicitly not a video-phase mechanism** — that is genlock. Someone will assume
otherwise, so the spec says it in bold.

## Consequences

- A cue cannot be delayed by an asset transfer or a preview stream, by
  construction rather than by prioritisation.
- Reconnect is the well-trodden path rather than the untested one, because
  everything about the protocol assumes it will happen.
- An operator's double-click cannot produce two takes.
- A late cue produces a clear refusal rather than a mistimed graphic.
- Three connections is more to build, monitor, and reason about than one.
- The replay ring is bounded, so a node away long enough falls back to a full
  resync — which is correct, and must be tested (spike S8).
- mDNS will not work in some networks and the manual path must therefore be a
  first-class UI, not a hidden fallback.

## Alternatives rejected

**Server dials the node.** Rejected: needs static addressing or discovery to be
authoritative, puts reconnect logic on the wrong side, and inverts the autonomy
model.

**gRPC bidirectional streaming.** The textbook answer. Rejected on toolchain
weight against a plain-ESM Express server, in a family with no protobuf.

**One connection for everything, with message prioritisation.** Rejected — it
makes a correctness property (a cue is never queued behind bulk data) into a
scheduling heuristic. Separate sockets make it structural.

**MQTT or a message broker.** Rejected: another service to run and keep alive on
a LAN appliance whose whole point is that it keeps working when things die.

**No discovery, manual configuration only.** Rejected against the North Star.

## Seam

`proto_ver` is negotiated at handshake, and the `at:` field exists unused so that
scheduled takes are additive rather than breaking. `pmg-proto` is one crate that
generates both ends from one definition, so a third-party node implementation has
an unambiguous contract to build against.

The three-connection split is also what would let a large installation put assets
behind a caching proxy and preview behind a media server without touching the
control path.
