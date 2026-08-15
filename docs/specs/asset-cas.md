# Spec — Content-addressed asset store

**Normative.** Decisions and reasoning:
[ADR 0006](../decisions/0006-content-addressed-assets.md).

## 1. Identity

Every asset is identified by `blake3:<64 lowercase hex>` of its exact bytes.

blake3 specifically: fast enough to hash a 4 GB video at ingest without annoying
anyone, and its tree structure gives cheap *verified* range fetches, which matters
for resuming a large transfer.

**Assets are immutable.** `logo.png` is not a name a node resolves; it is a hash a
node either has or does not. Replacing a logo produces a different hash, a
different template revision, and an explicit action to move Items onto it. A
changed asset never silently propagates to something on air.

## 2. On-disk layout

Identical on the server and on every node, so the same code and the same
verification pass work in both places.

```
<data>/cas/
  blobs/9f/2c/9f2c8a…            # two-level fan-out on the first 2 bytes
  tmp/<random>                   # in-progress writes only
  meta.sqlite                    # hash → size, kind, first_seen, last_used, refs
```

**Writes are atomic**: stream to `tmp/`, hash while writing, verify, then
`rename(2)` into `blobs/`. `rename` within a filesystem is atomic, so **a partial
file is never visible as complete.** A `tmp/` entry older than 24 h is garbage
from a crashed transfer and is removed at startup.

## 3. Ingest (server)

`POST /api/assets` (multipart) → `{ hash, size, kind, existed: bool }`.

Ingest:

1. Streams to `tmp/`, hashing as it goes. Never buffers a whole file in memory.
2. Sniffs the kind (`image`, `video`, `audio`, `font`) from content, not from the
   filename.
3. **Rejects variable-frame-rate video** with a clear message rather than
   accommodating it (R5).
4. **Rejects HDR-tagged media** in v1 with a clear message rather than mangling
   it ([ADR 0012](../decisions/0012-color-range-and-the-key-channel.md)).
5. **Offers a transcode to an all-intra intermediate** — ProRes 4444, DNxHR, or
   HAP with alpha — for any video destined for a cue. Long-GOP H.264 with
   B-frames is where frame-accurate seek goes to die; it is accepted for
   convenience playback and transcoded for anything that must hit a mark. The
   transcode's output is a *different asset* with its own hash.
6. Records colour tags as metadata. **Media carries its own tags; never assume
   709.**

`existed: true` means the bytes were already present — deduplication is free and
automatic across every template that references the same logo.

## 4. Serving

```http
GET /cas/blake3:9f2c…
Accept-Ranges: bytes
ETag: "blake3:9f2c…"
Cache-Control: public, max-age=31536000, immutable
```

Range requests and resume are **required**, not optional: a node interrupted at
3.9 GB of a 4 GB video must not start over.

Because content addresses are immutable, `immutable` caching is exactly correct,
and a large installation can front this with a caching proxy without any node
knowing.

## 5. Node staging

On receiving an `Assignment`, the node computes the set of hashes it lacks and
fetches them:

- Bounded concurrency (default 4), so staging cannot saturate the link.
- Per-blob retry with jittered backoff, forever. A transfer failure is never
  fatal and never blocks the control socket.
- **Every blob is verified against its hash after download.** A mismatch deletes
  and retries; three mismatches raise a hard error naming the hash, because that
  means something is corrupting data in transit or at rest.
- Progress is published as `SyncProgress` (PMG Link §4) and reaches browsers as
  the `node:<id>:staging` snapshot topic.

> **An Item is not armable until its node reports it ready.** Not "warns" — *not
> armable*. This is the enforcement point that makes "every byte before air" a
> property rather than a hope. The Show view greys the button and names the file
> and the bytes outstanding.

## 6. Retention and GC

**The node cache is permanent by default.** Not LRU-evicted. Disk is cheap; a
re-download on the morning of a service is not.

GC is an explicit operator action in Configure (`graphics.assets.manage`):

1. Compute the live set — every hash referenced by the current assignment plus
   every assignment retained in history (default: last 10).
2. Report what *would* be removed, with total bytes, **before** removing
   anything.
3. Remove only on confirmation.

Never GC automatically on low disk. A node that silently deletes an asset it will
need in an hour has turned a disk-space warning into an on-air failure. Low disk
is a health condition surfaced in Configure and on `node:<id>:telemetry`.

## 7. Export bundles

A `.pmgpkg` is a template revision plus the transitive closure of its asset
hashes, with a manifest:

```jsonc
{
  "pmgpkg_version": 1,
  "template_rev": "blake3:7c1e…",
  "include_fonts": true,          // §7.1
  "assets": [ { "hash": "blake3:9f2c…", "size": 88231, "kind": "image" } ]
}
```

Import verifies every hash on arrival before anything is added to the store.

### 7.1 `include_fonts` — the licensing seam

| Value | Behaviour |
|---|---|
| `true` | Font bytes are embedded. Fine in-house. **This is redistribution** and is not appropriate for a bundle that leaves the organisation. |
| `false` | Fonts are referenced by identity (family, weight, style, and a hash for verification). Import checks the fonts are present locally and **refuses with a list of what is missing** rather than silently substituting. |

One field, decided now, because adding it later would be a format break (R10,
[ADR 0005](../decisions/0005-the-pmg-scene-format.md) §Seam).

## 8. Verification

`pmgctl cas verify [--repair]` walks the store, re-hashes every blob, and reports
mismatches and orphans. `--repair` deletes mismatched blobs so they will be
re-fetched.

This runs in `--selftest` against a small fixture store, and it is the right thing
to run on a node that has done something inexplicable.
