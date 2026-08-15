# ADR 0009 — Nodes are autonomous: the survival invariant and where cue authority lives

Status: accepted · 2026-08-14

## Context

`docs/VISION.md` names three moments that define success. The third is the
hardest: *the control server gets restarted mid-service and nobody in the room
notices.*

Most systems fail this. The usual architecture makes the server the brain and the
renderer a display: the server holds "what is on air," pushes state, and the
renderer draws what it is told. It is a clean model and it has one property that
disqualifies it here — the show now depends on a piece of software that a person
might reasonably want to restart, on a network that might reasonably hiccup, in a
building where the person who could fix it is running audio.

There is a second, quieter question hiding behind the first. If a node must
survive without the server, and a hardware surface — a GPI from the switcher, a
Stream Deck, a MIDI button — must also work without the server, then those
surfaces cannot route through the server. Which means cue authority is genuinely
distributed, which means the server can no longer be the sole author of on-air
state. Those are the same problem, and answering one without the other produces a
system that is autonomous in the demo and not on the day.

## Decision

**A node holds everything it needs on local disk and keeps playing. The node is
the authority on what is on air.**

### The invariant

> **The node is the authority on what is on air.**
> **The server is the authority on content and assignment.**

On reconnect the server **reads** `StateReport` and adopts it. It never pushes
its idea of on-air state back down. If the server's rundown disagrees with what
the node reports, the UI shows the divergence and offers an explicit reconcile —
it never silently re-fires. This one rule is what makes autonomy real rather than
aspirational.

### What "holds everything" means

- **`Assignment` is acked only after it is persisted to disk.** That ordering is
  the whole invariant in one line: anything a node has acked, it can reconstruct
  from `assignment.cbor` with no server present.
- Every asset the assignment references is in the node's local CAS, verified,
  before any Item using it is armable (ADR 0006).
- On boot with no server: load the assignment, verify every hash, open the
  outputs, and **come up black**. Auto-restoring a graphic after a power cut is
  an unpleasant surprise on an unattended machine; restore is an explicit
  per-output opt-in.

### Where cue authority lives

**Hardware surfaces terminate at the node, not at the server.** GPI, MIDI, OSC,
and the node's own HTTP/WebSocket API on `:8520` all speak the same cue message
shapes directly into the node's layer engine. The node also serves a minimal
status page on that port — the emergency surface when the server is dead.

The consequence is unavoidable and must be embraced rather than worked around:
**the node must publish every state change it makes, and the server must treat
`StateReport` as truth.** The two halves of this ADR are the same rule.

### What is explicitly not in v1

**No node redundancy.** A dead node is a dead output. The accepted mitigation is a
spare machine and an operator who can move a cable. Automatic N+1 failover is its
own multi-month project, and distributed failover systems cause outages of their
own.

## Consequences

- The server can be restarted, upgraded, or moved mid-service. This is the point,
  and it is worth a great deal operationally.
- A network partition degrades the system to "you cannot change anything" rather
  than "everything goes black."
- Every hardware surface keeps working when the server is down, which is when an
  operator most needs one.
- **The server cannot assume it knows what is on air**, ever. Every screen that
  displays on-air state renders the node's report, not the server's intent. This
  is a discipline that has to hold in every feature.
- Reconciliation after a disconnect is a real UI with a real diff, not a spinner.
- Nodes need meaningful local disk for their CAS.
- Cue authority in more than one place means the cue log must record *which
  surface* fired each cue, or the log becomes confusing the first time someone
  uses a GPI.

## Alternatives rejected

**Server-authoritative state, node as display.** Simpler, one source of truth,
much easier reconciliation. Rejected because it fails `docs/VISION.md`'s third
moment, which is not negotiable.

**Node caches state but the server wins on reconnect.** A tempting middle path:
autonomy while disconnected, simplicity when connected. Rejected because "the
server wins" means the server re-asserts its idea of on-air state, and any
disagreement becomes a graphic changing on air at the moment the network comes
back — the worst possible time. Adoption plus an explicit reconcile is more work
and is correct.

**Hardware surfaces route through the server.** Simpler — one cue path, one
implementation. Rejected because it makes every hardware surface useless in
exactly the failure it exists to cover.

**Warm standby node, mirrored state, manual cut.** Genuinely attractive: it turns
a machine failure into a ten-second recovery. Rejected for v1 only on cost and
scope, not on merit. It is the first thing to add when redundancy becomes a
priority, and the architecture — content-addressed assets, persisted assignments,
idempotent cues — is already shaped to allow it.

## Seam

Everything a warm standby or an N+1 group would need is already true: assignments
are durable and content-addressed, assets are verifiable, cues are idempotent on
`(epoch, cue_id)`, and state is reported rather than assumed. Adding redundancy
later is a new coordination layer over an unchanged node, not a rewrite.

The node's own `:8520` API is also the seam for embedding: a node is usable as a
standalone renderer, driven by something that is not our server at all.
