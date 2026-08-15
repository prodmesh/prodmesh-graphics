# ADR 0006 — Content-addressed assets; a node holds every byte before air

Status: accepted · 2026-08-14

## Context

Fonts, images, and video have to reach the machines that render them. There are
three obvious ways to do that and one of them is what most houses actually do
today: put everything on an SMB share and let every machine mount it.

That works right up until it doesn't. The share becomes a component in the
program path — a network hiccup at the wrong moment is a missing video on air,
and a NAS that reboots takes every node with it. Worse, it is silent: nothing
tells you a node cannot see a file until the moment it needs it.

There is also a subtler problem that content-addressing solves and paths do not.
"Is Booth-1 ready for this service?" is a question an operator needs answered
*before* the service, with a yes or a no. With file paths, the honest answer is
"probably" — you can check that files exist, but not that they are the right
version, not that they are complete, and not that a designer did not replace
`logo.png` an hour ago.

## Decision

**Every asset is identified by `blake3:<hex>` of its contents. The control server
is the origin. Nodes pull what they are missing and cache it on local disk
forever.**

- Assets are immutable by construction. `logo.png` is not a name a node resolves;
  it is a hash a node either has or fetches. Replacing a logo produces a different
  hash, a different template revision, and an explicit action to move Items onto
  it.
- blake3 specifically: fast enough to hash a 4 GB video at ingest without
  annoying anyone, and its tree structure gives cheap *verified* range fetches
  later, which matters for resuming a large transfer.
- **Assets travel over plain HTTP** — `GET /cas/blake3:<hex>`, with Range and
  resume — **never over the control socket.** A 4 GB video must not
  head-of-line-block a take. This is the single most important framing decision
  in the whole transport design and it is why ADR 0007 has three connections
  rather than one.
- The node verifies each blob against its hash and renames it into place
  atomically. A partial file is never visible as complete.
- The node's cache is **permanent, not LRU-evicted by default.** Disk is cheap;
  a re-download the morning of a service is not. GC is an explicit operator
  action in Configure, with a report of what would be removed.
- **The enforcement point: an Item is not armable until its node reports
  `ready`.** Not "warns" — *not armable*. The Show view greys the button and
  names the file that is still transferring and how much is left. Staging
  progress is a snapshot topic, so any browser can watch it.

## Consequences

- "Is this node ready?" has an exact answer, computed from a set of hashes, and
  it is answerable on Saturday.
- A node that has staged an assignment can render it with no server, no share,
  and no network. That is the mechanism ADR 0009's autonomy invariant runs on.
- Two Items using the same logo transfer it once, across every template that
  references it.
- "What exactly was on air" is answerable exactly, because the template revision
  and every asset it used are immutable and named by content.
- **We are building an asset service** — ingest, hashing, storage, range serving,
  progress reporting, and GC. That is real work that a network share would have
  been free.
- Disk on both the server and every node grows monotonically until someone runs
  GC.
- A designer replacing an image does not "update it everywhere." That is
  deliberate — silent propagation of a changed asset to an on-air template is
  exactly the surprise this design refuses — but it will surprise people who
  expect share semantics, and the UI has to explain it.

## Alternatives rejected

**Shared network volume (SMB/NFS).** Trivial to build and matches existing
practice. Rejected because it puts a network file system in the program path,
gives no answer to "is this node ready," and fails silently and late. This is the
alternative most likely to be re-proposed when the asset service feels like a lot
of work in M1; the answer is that the first missing video on a Sunday costs more
than the whole service did.

**Push on assign.** The server pushes assets when a playlist is assigned. Simple,
explicit, with a clear ready state. Rejected as wasteful for large media, and
awkward when a node joins late or restarts — the node then depends on the server
choosing to re-push, rather than on its own reconciliation. Pull is the model
where the node can always fix itself.

**Embed small assets in the template, reference large ones.** Templates become
self-contained and portable, which genuinely helps the export-bundle story.
Rejected as two asset paths to build and test for a benefit that `.pmgpkg`
already provides at the boundary where it matters.

## Seam

Content-addressing is what makes a portable export bundle possible at all: a
`.pmgpkg` is a template revision plus the transitive closure of its hashes, and
it can be verified on arrival. That is the shape a customer would move between
installations, or that we would use to ship a template library.

**The font-licensing seam lives here too.** `.pmgpkg` carries
`include_fonts: bool` — embed the bytes (fine in-house) or reference fonts by
identity and require a local install (what a product would need). One field
decided now rather than a format break later. See R10.

The CAS API (`GET /cas/:hash` with Range) is deliberately dull and standard, so
it can be fronted by a CDN, a caching proxy, or a peer-to-peer distribution layer
in a large installation without any node knowing the difference.
