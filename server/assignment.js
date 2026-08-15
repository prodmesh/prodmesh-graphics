/**
 * Building the assignment a node is given.
 *
 * An assignment says **what a node must be able to play**. It does not say what
 * is playing — that is the node's answer, and nothing here may imply otherwise
 * (ADR 0009).
 *
 * # Why the digest is over a canonical shape
 *
 * `digest` exists so an already-current node can answer `AssignmentOk` with no
 * transfer. That only works if the same content always hashes the same, so the
 * payload is serialised with sorted keys before hashing. Hashing
 * `JSON.stringify` of an object built in whatever order the rows came back
 * would make the digest change for no reason, and a node would re-stage
 * everything on every reconnect while looking like it was working.
 *
 * # M1 scope
 *
 * Every published item goes to every node. Per-node scoping arrives with
 * playlists and the rundown in M2; inventing half of it now would mean two
 * assignment builders to reconcile later.
 */

import { createHash } from 'node:crypto'
import { handle } from './db.js'

/**
 * The stack a node gets before anyone has configured it.
 *
 * Bottom to top, which is also compositing order: a Bug sits over a Lower
 * Third, and a Fullscreen covers both.
 */
export const DEFAULT_LAYERS = ['Fullscreen', 'Lower Third', 'Bug']

/**
 * Colour for an output row, with the defaults filled in.
 *
 * 709 narrow is what an HD plant is — but "what it is" and "what nobody stated"
 * must not look the same on the wire, so the defaults travel explicitly rather
 * than being left for a node to assume.
 */
const colorOf = (o) => ({
  matrix: o.matrix ?? 'bt709',
  range: o.range ?? 'narrow',
  key_range: o.key_range ?? 'narrow',
  key_mode: o.key_mode ?? 'fill',
})

/** Field values → the protocol's closed DataValue set. */
function toWireData(data) {
  const out = {}
  for (const k of Object.keys(data).toSorted()) {
    const v = data[k]
    if (typeof v === 'number') out[k] = { Number: v }
    else if (typeof v === 'boolean') out[k] = { Bool: v }
    else if (typeof v === 'string' && /^blake3:[0-9a-f]{64}$/.test(v)) out[k] = { Asset: v }
    else out[k] = { Text: String(v ?? '') }
  }
  return out
}

/** Stable stringify: sorted keys at every level, so the digest means something. */
function canonical(value) {
  if (Array.isArray(value)) return `[${value.map(canonical).join(',')}]`
  if (value && typeof value === 'object') {
    return `{${Object.keys(value).toSorted().map((k) => `${JSON.stringify(k)}:${canonical(value[k])}`).join(',')}}`
  }
  return JSON.stringify(value ?? null)
}

/**
 * Build the assignment payload for a node.
 *
 * @param {object} opts
 * @param {string[]} opts.layers Layer names, in order. **By name, never index.**
 */
export function build({ nodeId = null, outputId = 'out-0', mode = '1920x1080', layers = DEFAULT_LAYERS } = {}) {
  const db = handle()

  // Configured outputs win. A node with none yet gets the default stack, so it
  // is usable the moment it connects rather than needing a trip to Configure
  // before anything can go on air.
  const configured = nodeId
    ? db.prepare('SELECT * FROM outputs WHERE node_id = ? ORDER BY output_id').all(nodeId)
    : []
  const outputs = configured.length
    ? configured.map((o) => ({
        id: o.output_id,
        mode: o.mode,
        layers: JSON.parse(o.layers),
        color: colorOf(o),
      }))
    // Defaults travel EXPLICITLY rather than being left for the node to assume.
    // 709 narrow is what an HD plant is, but "what it is" and "what nobody
    // stated" should not look the same on the wire.
    : [{ id: outputId, mode, layers, color: colorOf({}) }]

  const items = db
    .prepare(
      `SELECT i.id, i.label, i.template_rev, i.data, i.destination
         FROM items i ORDER BY i.id`,
    )
    .all()

  const revs = [...new Set(items.map((i) => i.template_rev))]
  const templates = revs.length
    ? db
        .prepare(
          `SELECT template_rev, name, min_reader, manifest FROM template_revisions
            WHERE template_rev IN (${revs.map(() => '?').join(',')})
            ORDER BY template_rev`,
        )
        .all(...revs)
    : []

  // Assets the templates declare. The SceneDoc itself is fetched by its
  // template_rev, which IS its content address — so it needs no separate entry.
  const assets = [
    ...new Set(templates.flatMap((t) => JSON.parse(t.manifest).assets ?? [])),
  ].toSorted()

  const payload = {
    gen: 0, // set by nodeLink, which owns the counter per node
    digest: '',
    outputs,
    templates: templates.map((t) => ({
      template_rev: t.template_rev,
      name: t.name,
      min_reader: t.min_reader,
    })),
    items: items.map((i) => {
      const dest = JSON.parse(i.destination)
      // An item names its destination; falling back to the first output and
      // its bottom layer keeps a half-configured rundown usable.
      const out = outputs.find((o) => o.id === dest.output) ?? outputs[0]
      return {
        id: i.id,
        label: i.label,
        template_rev: i.template_rev,
        data: toWireData(JSON.parse(i.data)),
        output: out?.id ?? outputId,
        layer: dest.layer ?? out?.layers[0] ?? null,
      }
    }),
    assets,
  }

  // Over everything except gen and digest: those are per-node bookkeeping, and
  // including them would make identical content hash differently per node.
  const { gen: _g, digest: _d, ...content } = payload
  // sha256, not blake3, and labelled honestly. The node only ever compares
  // this for equality — it is not a content address and must not be mistaken
  // for one, which is exactly why it does not wear a `blake3:` prefix.
  payload.digest = `sha256:${createHash('sha256').update(canonical(content)).digest('hex')}`
  return payload
}
