# ADR 0005 — The PMG Scene format: box tree, bindings, timelines, versioning

Status: accepted · 2026-08-14

## Context

The scene format is the most consequential thing in the repository. It is what
designers' work *is*. A renderer can be rewritten; a protocol can be versioned; a
UI can be replaced. Templates authored against a format are the asset that has to
survive all of that, and — per `docs/VISION.md` — the asset a customer would
someday own.

Four questions have to be answered together, because they constrain each other:
how layout is expressed, how data binds, how animation is described, and how any
of it survives a version change.

The layout question has a specific requirement behind it: *a lower third that
shrinks and grows with the text*. That is the canonical broadcast responsive
case and it has to be cheap, because it happens on every take.

## Decision

Media type `application/vnd.prodmesh.scene+cbor`. Files: `.pmgscene` (compiled),
`.pmgtemplate` (scene + manifest), `.pmgpkg` (export bundle with assets). Full
normative detail in `docs/specs/scene-format.md`; this records the choices.

### Two representations, one schema

**Canonical CBOR** — RFC 8949 core-deterministic: definite lengths, sorted keys,
shortest integer encoding, f32 where exact. This is what gets hashed and shipped,
and determinism is non-negotiable because `template_rev` *is* the hash.

**A JSON mirror** with the same field names, for diffs, git, hand-editing, and
docs. `pmgctl scene fmt` round-trips losslessly, guarded by a property test.

Not JSON on the wire: float precision loss on keyframes, no binary blobs, and
three to five times larger on a keyframe-heavy document.

### Layout: a flexbox box tree

Nodes carry `layout` (direction, justify, align, wrap, grow, shrink, basis,
sizing, margin, padding, gap, position, inset, aspect), `transform` (translate,
rotate, scale, anchor, skew), and `style`. Units are `Px` in design space,
`Percent`, `Auto`, and `Content`.

**The distinction that makes the responsive lower third cheap:** `layout.*`
affects siblings and forces a relayout; `transform.*` does not. So a lower third
*sizes* itself once, when data binds, using `width: Content` in a row — and
*reveals* itself with a transform and a clip, which is free per frame. The format
makes this structurally visible and the editor warns when a timeline targets a
layout property. This is the Xpression behaviour, arrived at honestly.

### Node ids are stable forever

**Timeline tracks address `node_id/property`.** Ids are assigned once and never
renumbered; renaming a layer changes its display name only. Reordering the tree,
regrouping, or renaming can therefore never break an animation. This is the
single most compatibility-critical decision in the format.

### Bindings: a total expression language, not scripting

```jsonc
"fields": [{ "key":"name", "type":"text", "label":"Name", "required":true, "max_len":40 }]

"content":    { "kind":"text", "binding": { "expr":"${name}",
                                            "transform":["trim","ellipsize:40"] } },
"visible_if": { "field":"title", "op":"non_empty" }
```

`transform` is a **fixed enum** of pure string operations — `trim`, `upper`,
`lower`, `title`, `truncate:n`, `ellipsize:n`, `pad:n`. Every operation is total:
defined on every input, incapable of failing. This evaluates on the render thread
at air time, so "cannot fail" is a requirement, not a nicety.

**`visible_if` is what collapses a lower third to one line when there is no
title.** That single field carries more operator value than any amount of
interpolation syntax, and it cannot be expressed with interpolation alone.

### Timelines: named, with keyframe tracks

```jsonc
{ "name": "In", "duration_ms": 800, "on_complete": "hold", "loop": false,
  "tracks": [ { "target": "title/transform.translate.x", "blend": "replace",
                "keys": [ {"t":0, "v":-400, "ease":{"cubic":[0.2,0,0,1]}},
                          {"t":800, "v":0} ] } ] }
```

`In` and `Out` are reserved; everything else is user-named and free — `Update`,
`Highlight`, `Loop`. Multiple running timelines evaluate in start order, **last
writer per target wins per frame** — documented, deterministic, and no implicit
blending in v1. The `blend` field exists now and only `replace` is implemented,
because adding the field later is a break.

### Versioning

- `format_version.major` breaks; `minor` adds. A reader accepts a document whose
  `min_reader <= reader_major`.
- **Unknown map keys and unknown node kinds are preserved on read and re-emitted
  on write.** An older designer can open, edit, and re-save a newer document
  without silently deleting what it does not understand. This is the difference
  between a format that survives two versions in the field and one that does not.
- **But it must refuse to *render* an unknown kind** — magenta placeholder box
  plus `degraded: true` on the instance. Rendering nothing and looking fine is
  the dangerous behaviour.
- **A node refuses a template whose `min_reader` exceeds its own at assignment
  time, loudly** — not at take time. You find out at 10am Saturday, not 10:15
  Sunday. This rule is worth more in practice than any other compatibility
  mechanism here.
- Migration is a chain of one function per version step in `pmg-scene::migrate()`,
  each with a frozen fixture in `testdata/migrations/`. **A shipped fixture is
  never edited.**
- Migrating produces a new `template_rev`; Items pin revisions, so migration never
  changes anything on air.

### Assets, including fonts

Every asset is `blake3:<hex>` — fast enough to hash a 4 GB video at ingest, with a
tree structure that gives cheap verified range fetches later. **Fonts are
assets.** That is what makes WYSIWYG survive across machines. A scene may name a
system font only through an explicit fallback chain, and the designer flags it:
*this will differ between machines.*

## Consequences

- Revisions are immutable and content-addressed, so "which template was on air"
  is answerable exactly, forever.
- The cheap-animation path is the obvious one, and the expensive one is visibly
  marked.
- Animations survive tree refactoring, which designers do constantly.
- Templates cannot fail at air time from a binding, because bindings cannot fail.
- Canonical CBOR is fussy to implement correctly and the round-trip property test
  is load-bearing. If it regresses, revisions stop being stable.
- The fixed transform enum will feel restrictive within a year. That is intended;
  see below.
- Preserve-unknown-on-write means the reader carries data it does not understand,
  which complicates the in-memory model slightly and is worth it.

## Alternatives rejected

**Constraint solver (Cassowary) layout.** More expressive than flexbox, handles
things flex cannot. Rejected because authoring UI for constraints is genuinely
hard and under/over-constrained layouts are a confusing failure mode for a
designer working under time pressure. Flexbox is a model every designer already
has in their head from CSS.

**Anchors and text-fit rules only.** Simpler and very predictable. Rejected as
too weak for tickers, tables, and stat panels, all of which are on the road map.

**Per-property enter/exit animation rules.** Simplest to build. Rejected because
staggered choreography across many elements — which is most of what makes a
graphic look professional — becomes painful to author.

**Scriptable timelines (Rhai/Lua).** Maximum expressiveness. Rejected as the
direct enemy of authoring stability: templates become programs that can break on
air. If a case ever genuinely requires it, that is its own ADR with its own
failure story, and a sandbox with a fuel limit — not a quiet extension of
`transform`.

**JSON on the wire.** Rejected on float precision, binary blobs, and size.

## Seam

This ADR *is* mostly seam. `format_version` + `min_reader`, unknown-field
preservation, frozen migration fixtures, and immutable content-addressed
revisions exist so a customer's templates open in three years.

**Font licensing:** embedding font bytes in an export bundle is redistribution —
fine in-house, not fine productized. The seam is one field on `.pmgpkg`:
`include_fonts: bool`, versus reference-by-identity with a require-local-install
check. One field now; a format break later. See R10.
