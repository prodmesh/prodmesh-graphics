# ADR 0008 — Snapshots stream, cues are acked: two paths for live data

Status: accepted · 2026-08-14

## Context

ProdMesh-Control solved live data for a dashboard and wrote down exactly where
its solution stops working. From `server/streamHub.js`:

> Under backpressure the hub keeps at most **one pending frame per topic per
> connection**, newest wins, flushed on `drain`. This is correct *because every
> topic carries a state snapshot*: a client only ever renders the newest value,
> so superseded ones cost nothing to drop and the final state is still right. It
> would be wrong for an event-shaped topic ("a cue fired"), where a superseded
> value is a lost event — nothing publishes one today, and **anything that does
> needs its own path**.

This system publishes them constantly. A take is an event. A superseded take is
not a stale value that a newer one corrects; it is a graphic that never went on
air, and nothing downstream will ever notice.

The reason conflation exists in the first place is worth keeping in view, because
it is not a design flaw to route around. `res.write()` returns false once the
socket buffer fills, and Node then buffers without bound — so an endpoint that
ignores backpressure is a memory-exhaustion vector, and more mundanely a booth
Mac that goes to sleep costs the server memory for as long as it sleeps.
Conflation is the right answer for snapshots and we are keeping it.

There is a third thing that muddies the line, and it is where a naive design goes
wrong: **operators want to see a feed of takes in the browser**. "Lower Third ·
taken 10:15:03.12 · frame 1841944." That is event-shaped data that a browser
wants over SSE, which is the conflating transport. The temptation is to publish
`{event: "take", …}` on a topic and move on.

## Decision

**Two paths, and one rule that bridges them.**

### Snapshot-shaped → SSE topics, conflate freely

`node:<id>:health`, `node:<id>:telemetry`, `node:<id>:sync`,
`output:<id>:layers`, `item:<id>:readiness`, `rundown:<id>:cursor`,
`preview:<id>:stats`.

Every one is idempotent and the newest value is the whole truth. `streamHub.js`
comes over from ProdMesh-Control essentially unchanged, including refcounted
producers — nobody polls a node that no browser is watching — and the
`valid(params)` guard, because subscribing *starts work* and an endpoint that
accepts any string is a resource-exhaustion primitive.

### Event-shaped → PMG Link, acked

`take`, `take-out`, `update`, `clear-layer`, `clear-all`, `timeline-jump`,
`playlist-advance`.

Ordered by `seq`, idempotent on `(epoch, cue_id)`, acked, deadline-bounded. ADR
0007.

### The bridge

> **An event stream reaches the browser as the snapshot of a log tail.**

Topic `show:<id>:cuelog` publishes `{ head_seq, entries[] }` — a sliding window
of the last N entries, not one entry per frame. Conflation becomes safe again,
because the newest frame *contains* everything the dropped frames carried. A
client that has been away sends `?since=<seq>` and gets the gap over plain HTTP.

**Never publish a bare `{event: "take", item: …}` frame on a conflating topic.**
This is the general escape from the trap `streamHub.js` warns about, and it is
worth stating as a rule rather than solving once per feature.

### And the operator-facing corollary

**`POST /api/cue` does not return until the node acks or `deadline_ms` expires.**
On expiry it returns `503 cue_not_acked` and the Show view says so in words. A
fire-and-forget button on a broadcast system is worse than no button, because it
teaches an operator to trust something that is not true.

## Consequences

- A dropped SSE frame can never be a missed graphic. The two failure modes stay
  separated by construction.
- Adding a new live *value* is cheap: register a topic and it inherits
  refcounting, snapshots for joiners, and backpressure handling.
- Adding a new *event* is deliberately not cheap: it goes on PMG Link, needs a
  `seq`, an ack, an idempotency key, and a deadline. That friction is the point.
- Operators get a truthful take log with gap recovery, over the same single SSE
  connection everything else uses.
- The cue log is now a durable server-side structure with a sequence number and a
  retention window, not a convenience.
- Two mechanisms means someone will eventually put an event on a topic. The rule
  is in `AGENTS.md`'s gotchas and it is the kind of thing to catch in review.
- A synchronous cue endpoint holds an HTTP request open for up to the deadline.
  Bounded at 500 ms for takes, and worth it.

## Alternatives rejected

**Everything over WebSocket, no SSE.** Uniform, and cues and snapshots share a
path. Rejected because ProdMesh-Control's reason for choosing SSE still applies —
a LAN appliance with no TLS gets no HTTP/2, so the browser's six-connections-per-
origin limit is real and unfixable at this layer — and because uniformity is the
problem, not the solution: it invites treating a cue like a value.

**Sequence numbers on SSE topics with client-side gap detection.** Would make
topics event-safe. Rejected: it reimplements a reliable channel over a
deliberately lossy one, and the client would still have to fetch gaps over HTTP —
at which point the log-tail rule is the same mechanism with less machinery and a
clearer contract.

**Fire-and-forget cues with optimistic UI.** Feels instant. Rejected outright: an
operator must be able to trust that the button did what it said.

## Seam

The topic catalogue and the `?since=<seq>` gap convention are documented in
`docs/specs/http-api.md` as public API. Any client — ProdMesh-Control, a
Companion module, a DashBoard panel, a customer's own dashboard — gets the same
guarantees and the same recovery path.

The log-tail rule is the reusable part. It is worth carrying back to
ProdMesh-Control the first time that app needs an event feed of its own.
