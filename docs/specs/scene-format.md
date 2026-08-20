# Spec — The PMG Scene format

**Normative.** This is the format of record. Where this document and an
implementation disagree, this document is right and the implementation is a bug.

Decisions and reasoning: [ADR 0005](../decisions/0005-the-pmg-scene-format.md).

| | |
|---|---|
| Media type | `application/vnd.prodmesh.scene+cbor` |
| Compiled scene | `.pmgscene` |
| Template (scene + manifest) | `.pmgtemplate` |
| Export bundle (with assets) | `.pmgpkg` |
| Current version | `1.0`, `min_reader: 1` |

## 1. Encoding

### 1.1 Canonical CBOR is the format of record

RFC 8949 core-deterministic encoding:

- Definite-length arrays and maps only.
- Map keys sorted by their encoded bytes.
- Shortest-form integer encoding.
- Floats encoded as `f32` when the value round-trips exactly through `f32`,
  otherwise `f64`. No `f16`.
- No indefinite-length strings, no tags outside those listed in §1.3.

**Determinism is not optional.** `template_rev` is `blake3` of these bytes, so a
non-deterministic encoder makes revisions unstable and every guarantee built on
them false.

### 1.2 The JSON mirror

Every document has an equivalent JSON form using the same field names, for diffs,
version control, hand-editing, and documentation. `pmgctl scene fmt` converts in
both directions **losslessly**, and a property test asserts the round trip. All
examples in this document are shown in the JSON mirror.

### 1.3 Tags

| Tag | Meaning |
|---|---|
| 2 / 3 | Bignum — MUST NOT appear; reject |
| (none others) | Any other tag is an error |

Binary blobs use CBOR byte strings directly.

## 2. Document structure

```jsonc
{
  "format": "pmg.scene",
  "format_version": { "major": 1, "minor": 0 },
  "min_reader": 1,
  "id": "01J8ZK...",                     // ULID, stable across revisions
  "name": "Lower Third — Speaker",
  "canvas": {
    "width": 1920, "height": 1080,
    "safe": { "action": 0.93, "title": 0.90 }
  },
  "fields":    [ /* §5 */ ],
  "fonts":     [ /* §7 */ ],
  "assets":    [ /* §7 */ ],
  "root":      { /* §3 */ },
  "timelines": [ /* §6 */ ],
  "ext":       {}                        // forward-compat escape hatch
}
```

`canvas` is **design space**. A template designed at 1920×1080 and played out at
2160 is rendered *at* 2160 — geometry scales, rasterization does not. Never
upscale a 1080 raster to 2160; glyph antialiasing gives it away immediately.

## 3. Nodes

```jsonc
{
  "id": "title",                 // STABLE — see §3.1
  "display_name": "Title line",  // free to change
  "kind": "box",                 // box | text | image | video | shape | mask | group
  "layout":    { /* §4 */ },
  "transform": { "translate": [0,0], "rotate": 0, "scale": [1,1],
                 "anchor": [0.5,0.5], "skew": [0,0] },
  "style":     { "fill": {...}, "stroke": {...}, "radius": 8,
                 "shadow": {...}, "blur": 0, "opacity": 1.0 },
  // `stroke` and `shadow` decorate the node's silhouette, and WHERE depends on
  // what the silhouette is. A box strokes INSIDE its edge (layout is never
  // grown) and knocks its shadow out under its own silhouette, like CSS
  // box-shadow. Text strokes OUTWARD — an inside stroke eats thin letterforms
  // — and its shadow shows through a translucent fill, like CSS text-shadow.
  // Neither effect ever changes layout; both are ink past the geometry.
  //
  // `style.gradient` (format 1.1) draws a linear gradient INSTEAD of `fill`:
  //   { "angle_deg": 180, "stops": [ { "color": {...}, "at": 0.0 }, ... ] }
  // The angle is CSS's dial — 0° up, clockwise, 90° left-to-right. 2 to 4
  // stops, `at` ascending in 0..=1, interpolated in LINEAR light (mixing
  // encoded values grows grey through the middle). The gradient line runs
  // through the rect's centre and spans its projection, as CSS sizes one.
  // Writers keep `fill` equal to the first stop: a 1.0 reader ignores the
  // key it does not know and renders that flat colour instead of nothing.
  "clip": false,
  "visible_if": { /* §5.3 */ },
  "content": { /* kind-specific */ },
  "children": [ /* Node */ ]
}
```

### 3.1 Node ids are stable forever

> **A node's `id` is assigned once and never changes.** Renaming a layer changes
> `display_name`. Reordering, regrouping, or moving a node in the tree does not
> change its `id`.

Timeline tracks address `id/property` (§6.2). This rule is what makes an
animation survive a designer refactoring the tree, and it is the most
compatibility-critical rule in the format.

Ids are unique within a document. The compiler rejects duplicates.

### 3.2 `layout` versus `transform` — the performance contract

| | Affects siblings? | Cost |
|---|---|---|
| `layout.*` | Yes — forces a relayout of the subtree and its siblings | **Per-frame if animated** |
| `transform.*` | No — applied after layout | Free |

**Animate `transform`.** The editor MUST warn when a timeline track targets a
`layout.*` property, and the compiler MUST record the warning in the manifest.

This is what makes the responsive lower third cheap: it *sizes* once, when data
binds (`width: "Content"` in a row), and *reveals* with a transform and a clip.

### 3.3 Content by kind

```jsonc
// text
"content": { "kind": "text",
             "binding": { /* §5 */ },
             "font": { "family": "Inter", "weight": 600, "size": 44,
                       "features": ["kern","liga","tnum"], "tracking": 0,
                       "line_height": 1.15 },
             "align": "start", "vertical_align": "center",
             "wrap": "word", "max_lines": 2, "overflow": "ellipsize" }

// image / video
"content": { "kind": "image", "asset_key": "bg", "fit": "cover" }
"content": { "kind": "video", "asset_key": "sting", "fit": "contain",
             "loop": false, "audio": { "layer_gain_db": 0.0 } }

// image / video, FIELD-DRIVEN: the media comes from this field's value in the
// Item's data — a content hash — and `asset_key` is ignored. One template,
// and every Item picks its own picture or clip. The field must be declared
// (§2) with the matching type; an Item that has not chosen renders the slot
// as degraded, honestly, rather than blank.
"content": { "kind": "video", "field": "clip", "asset_key": "", "fit": "cover",
             "loop": false }

// shape
"content": { "kind": "shape", "shape": "rect" }   // rect | ellipse | path
```

`features` is an OpenType feature list. **`tnum` matters** — clocks and scores
jitter without tabular figures.

### 3.4 Unknown kinds and unknown keys

- A reader **MUST preserve** unknown map keys and unknown node `kind`s on read
  and re-emit them byte-identically on write. An older designer can open, edit,
  and re-save a newer document without silently deleting what it does not
  understand.
- A renderer **MUST refuse to render** an unknown `kind`: draw a magenta
  placeholder box and set `degraded: true` on the `SceneInstance`. Rendering
  nothing and looking fine is the dangerous behaviour.

## 4. Layout

The Yoga flexbox model. Fields:

```jsonc
"layout": {
  "direction": "row",            // row | column | row_reverse | column_reverse
  "justify": "start",            // start | center | end | space_between | space_around | space_evenly
  "align": "stretch",            // start | center | end | stretch | baseline
  "wrap": "no_wrap",
  "grow": 0, "shrink": 1, "basis": "Auto",
  "width": "Content", "height": "Auto",
  "min_width": null, "max_width": {"Px": 900},
  "margin":  { "top": 0, "right": 0, "bottom": 0, "left": 0 },
  "padding": { "top": 12, "right": 24, "bottom": 12, "left": 24 },
  "gap": 8,
  "position": "relative",        // relative | absolute
  "inset": null,
  "aspect": null
}
```

### 4.1 Units

| Unit | JSON | Meaning |
|---|---|---|
| Pixels | `{"Px": 240}` | Design-space pixels |
| Percent | `{"Percent": 50}` | Of the parent's resolved main/cross size |
| Auto | `"Auto"` | Flexbox default behaviour |
| **Content** | `"Content"` | **Size to the measured content** — the responsive case |

`"Content"` on a text node's `width` calls `TextMeasure` and sizes the box to the
shaped text. That, in a `row` with `gap` and `padding`, is the entire
shrink-and-grow lower third.

### 4.2 Layout is pure

`pmg-layout` takes a tree and a `TextMeasure` implementation and returns
resolved rectangles. No I/O, no ambient state, no wall clock. Same input, same
output, on every platform and both targets — this is the half of ADR 0001's
promise that *is* guaranteed.

## 5. Fields and bindings

### 5.1 The field schema

```jsonc
"fields": [
  { "key": "name",  "type": "text",  "label": "Name",  "required": true,
    "max_len": 40, "sample": "Pastor Dan Whitfield" },
  { "key": "title", "type": "text",  "label": "Title", "sample": "Lead Pastor" },
  { "key": "photo", "type": "image", "label": "Photo", "optional": true }
]
```

Types: `text`, `number`, `bool`, `image`, `video`, `color`, `enum`.

`sample` is what Design previews with and what a golden image renders. An Item
supplying data MUST satisfy `required` and MUST NOT exceed `max_len`; the server
validates against the manifest's copy of this schema at Item save time.

### 5.2 Binding expressions — total by construction

```jsonc
"binding": { "expr": "${name}", "transform": ["trim", "ellipsize:40"] }
```

`expr` is a template string containing literal text and `${field_key}`
references. An unknown key resolves to the empty string. **There is no
arithmetic, no conditionals, and no function calls.**

`transform` is an ordered list from a **fixed enum**:

| Transform | Effect |
|---|---|
| `trim` | Strip leading and trailing whitespace |
| `upper` / `lower` / `title` | Case conversion (Unicode-aware) |
| `truncate:n` | Hard cut to *n* grapheme clusters |
| `ellipsize:n` | Cut to *n* grapheme clusters, appending `…` if cut |
| `pad:n` | Pad with spaces to *n* grapheme clusters |

**Every operation is total: defined on every input, incapable of failing.** This
evaluates on the render thread at air time, where "cannot fail" is a requirement.
A template must never be able to throw on air.

Adding a transform is a `minor` version bump. Adding *evaluation* — arithmetic,
conditionals, scripting — is a different format and needs its own ADR.

### 5.3 `visible_if`

```jsonc
"visible_if": { "field": "title", "op": "non_empty" }
```

Ops: `non_empty`, `empty`, `eq:<literal>`, `ne:<literal>`, `gt:<number>`,
`lt:<number>`, `true`, `false`.

A node whose `visible_if` is false is **removed from layout**, not merely hidden —
which is what collapses a two-line lower third to one line when there is no
title. That behaviour cannot be expressed by interpolation, and it is why this
field exists.

## 6. Timelines

```jsonc
{
  "name": "In",
  "duration_ms": 800,
  "on_complete": "hold",          // hold | evict | goto:<name>
  "loop": false,
  "tracks": [
    { "target": "title/transform.translate.x",
      "blend": "replace",         // replace | add   (only `replace` in v1)
      "keys": [
        { "t": 0,   "v": -400, "ease": { "cubic": [0.2, 0.0, 0.0, 1.0] } },
        { "t": 800, "v": 0 }
      ] }
  ]
}
```

### 6.1 Reserved names

`In` and `Out` are reserved and have engine meaning: `Take` plays `In`,
`TakeOut` plays `Out`. Every other name is free — `Update`, `Highlight`, `Loop` —
and is played by an explicit `Timeline{name, action}` cue.

### 6.2 Targets

`<node_id>/<property path>`, e.g. `title/transform.translate.x`,
`bar/style.opacity`, `photo/transform.scale`. Addressing by stable id (§3.1) is
what makes animations survive tree edits.

A track whose target node does not exist is **dropped with a compiler warning**,
never a hard error — deleting a layer must not make a template unopenable.

### 6.3 Evaluation

- `t` is milliseconds from the timeline's start. Keys are sorted; the compiler
  rejects duplicates at the same `t` on one track.
- Between keys, interpolate with the **preceding** key's `ease`. Easings:
  `linear`, `{"cubic":[x1,y1,x2,y2]}`, `{"steps":n}`.
- Before the first key, hold the first value; after the last, hold the last.
- **Multiple running timelines evaluate in start order; last writer per target
  wins per frame.** Documented, deterministic, no implicit blending in v1. The
  `blend` field exists so `add` is additive later rather than a break.
- **Evaluation is pure.** Given a timeline and a `t`, the result depends on
  nothing else. No wall clock inside `pmg-anim`.

### 6.4 Frame-boundary timing

The engine computes `t` from the output's exact rational frame rate:

```
t_ms = frames_since_start * 1000 * fps_den / fps_num
```

`60000/1001`, never a `59.94` float. An 800 ms `In` looks identical at 25p and at
59.94p.

## 7. Assets and fonts

```jsonc
"assets": [ { "key": "bg",   "hash": "blake3:9f2c…", "kind": "image" } ],
"fonts":  [ { "family": "Inter", "weight": 600, "style": "normal",
              "asset": "blake3:41ab…" } ]
```

Nodes reference assets by `key` locally; the `hash` is the global identity
(ADR 0006). Every hash in a document must resolve in the node's CAS before any
Item using the template is armable.

**Fonts are assets.** That is what makes WYSIWYG survive across machines. A scene
MAY name a system font only through an explicit fallback chain:

```jsonc
{ "family": "Helvetica Neue", "system_fallback": true }
```

and Design MUST flag it: *this will differ between machines*.

## 8. Compilation and the manifest

The compiler (`pmg-scene`, run in the browser as WASM and in `pmgctl` natively —
the same code) turns a Template Document into a SceneDoc plus a JSON manifest
sidecar:

```jsonc
{
  "template_rev": "blake3:7c1e…",
  "template_id": "01J8ZK…",
  "name": "Lower Third — Speaker",
  "format_version": { "major": 1, "minor": 0 },
  "min_reader": 1,
  "fields": [ /* copy of §5.1, for server-side Item validation */ ],
  "timelines": ["In", "Out", "Update"],
  "assets": ["blake3:9f2c…", "blake3:41ab…"],
  "warnings": [ { "code": "layout_animated", "node": "bar" } ]
}
```

**The control server validates the manifest and nothing else.** It never parses a
SceneDoc (ADR 0004). `template_rev = blake3(canonical CBOR of the SceneDoc)` and
revisions are immutable.

## 9. Versioning and compatibility

### 9.1 Reader rules

| Condition | Behaviour |
|---|---|
| `min_reader <= reader_major` | Read it |
| `min_reader > reader_major` | **Refuse, loudly, at assignment time** — never at take time |
| Unknown key or unknown `kind` | Preserve on read, re-emit on write (§3.4) |
| Unknown `kind` at render | Magenta placeholder, `degraded: true` |
| Unknown `transform` or `visible_if` op | Compile error in Design; at runtime, treat as identity / `true` and set `degraded` |

The assignment-time refusal is worth more in practice than every other
compatibility mechanism here. You find out at 10am Saturday, not at 10:15 Sunday.

### 9.2 Version bumps

- **`minor`** — new optional fields, new node kinds, new transforms, new easings.
  `min_reader` unchanged.
- **`major`** — anything a previous reader would misinterpret. `min_reader` is
  raised.

Current version: **1.1**, which introduced `style.gradient`. A 1.0 reader
ignores the field and renders the flat `fill` writers keep beside it.

### 9.3 Migration

`pmg_scene::migrate()` is a chain of one function per version step. Each step has
a frozen fixture in `testdata/migrations/<from>-to-<to>/`.

> **A shipped migration fixture is never edited.** If it is wrong, add a new step.
> Editing one silently changes what "we could read v1.0" means.

Migrating produces a **new `template_rev`**. Items pin revisions, so migration
never changes anything on air; moving an Item to a migrated revision is an
explicit operator action with a diff.

## 10. Conformance

An implementation conforms if it:

1. Round-trips canonical CBOR ⟷ JSON mirror losslessly (property test).
2. Produces byte-identical CBOR for the same logical document on every platform.
3. Preserves unknown keys and kinds across a read/write cycle.
4. Refuses to render an unknown kind, and marks the instance `degraded`.
5. Refuses a document whose `min_reader` exceeds its own, at assignment time.
6. Evaluates timelines purely, from exact rational frame time.
7. Passes every fixture in `testdata/migrations/`.
8. Passes `pmgctl --selftest`.
