/**
 * `POST /api/cue` — the operative endpoint, and the one place the whole system
 * is judged from a booth.
 *
 * # It is synchronous, on purpose
 *
 * It does not return until the node acks or the deadline expires. A
 * fire-and-forget button on a broadcast system is worse than no button, because
 * it teaches an operator to trust something untrue. On expiry it returns
 * **503 `cue_not_acked`** and the Show view renders that as words, not a spinner
 * that eventually gives up.
 *
 * # `cue_id` is the caller's responsibility
 *
 * Generated where the human acted — at button-press, at key-down, at the GPI
 * edge — and reused on retry. It is carried unchanged to the node, which dedupes
 * on `(epoch, cue_id)`. **A client that mints a fresh id per retry has defeated
 * the mechanism**, so a missing one is a 422 rather than something we invent
 * here: inventing it would make every retry a new cue and the bug would only
 * show up as a double-take on air.
 *
 * Spec: `docs/specs/http-api.md` §2.
 */

import express from 'express'
import { handle } from '../db.js'
import * as link from '../nodeLink.js'
import { append } from '../cueLog.js'

export const router = express.Router()

const fail = (res, status, error, label, extra) =>
  res.status(status).json({ error, label, ...extra })

const SURFACES = new Set([
  'show_view', 'companion', 'rosstalk', 'gpi', 'osc', 'midi', 'api', 'node_local',
])

/**
 * Build the CBOR op from the request. Layers are addressed **by name, never by
 * index** — an index silently re-points at a different graphic the moment
 * someone reorders the stack.
 */
function buildOp(body, item) {
  const output = body.output
  const layer = body.layer
  switch (body.op) {
    case 'take':      return [{ Take: { item: item.id, output, layer } }, null]
    case 'arm':       return [{ Arm: { item: item.id, output, layer } }, null]
    case 'take_out':  return [{ TakeOut: { output, layer } }, null]
    case 'update':    return [{ Update: { output, layer, data: toWireData(body.data ?? {}) } }, null]
    case 'clear_layer': return [{ ClearLayer: { output, layer } }, null]
    case 'clear_all': return [{ ClearAll: { output } }, null]
    case 'preload':   return [{ Preload: { item: item.id } }, null]
    case 'timeline':  return [{ Timeline: { output, layer, name: body.timeline ?? 'In',
                                            action: timelineAction(body.action) } }, null]
    default:          return [null, `There is no cue called “${body.op}”.`]
  }
}

const timelineAction = (a) =>
  a === 'stop' ? 'Stop' : typeof a === 'number' ? { Goto: { t_ms: a } } : 'Play'

/**
 * JSON field values → the protocol's closed `DataValue` set.
 *
 * Closed on purpose: an Item must not be able to smuggle a type the renderer
 * has no case for. A content address is an Asset even though it arrives as a
 * string, because that is what it is.
 */
function toWireData(data) {
  const out = {}
  for (const [k, v] of Object.entries(data)) {
    if (typeof v === 'number') out[k] = { Number: v }
    else if (typeof v === 'boolean') out[k] = { Bool: v }
    else if (typeof v === 'string' && /^blake3:[0-9a-f]{64}$/.test(v)) out[k] = { Asset: v }
    else out[k] = { Text: String(v ?? '') }
  }
  return out
}

const OPS_NEEDING_ITEM = new Set(['take', 'arm', 'preload'])

async function dispatch(body, ctx) {
  const db = handle()

  if (!body?.cue_id) {
    return { http: 422, payload: { error: 'validation_failed', label: 'This cue has no cue_id. Generate one where the button was pressed.' } }
  }
  const surface = body.surface ?? 'api'
  if (!SURFACES.has(surface)) {
    return { http: 422, payload: { error: 'validation_failed', label: `Unknown control surface “${surface}”.` } }
  }

  let item = { id: null, label: null }
  if (OPS_NEEDING_ITEM.has(body.op)) {
    const row = db.prepare('SELECT id, label FROM items WHERE id = ? OR take_id = ?').get(body.item, body.item)
    if (!row) {
      return { http: 409, payload: { error: 'no_such_item', label: 'That item is not on this server.' } }
    }
    item = row
  }

  const [op, problem] = buildOp(body, item)
  if (problem) return { http: 422, payload: { error: 'validation_failed', label: problem } }

  const ack = await link.cue(body.node, {
    cue_id: body.cue_id,
    op,
    deadline_ms: body.deadline_ms,
  })

  // The log records what happened AND which surface did it — without that the
  // log is confusing the first time someone uses a GPI (ADR 0014).
  const entry = append(body.show ?? 'default', {
    cue_id: body.cue_id,
    surface,
    op: body.op,
    node: body.node,
    output: body.output ?? null,
    layer: body.layer ?? null,
    item: item.id,
    item_label: item.label,
    user: ctx.user,
    station: ctx.station,
    fired_at_frame: ack.fired_at_frame ?? null,
    result: ack.result,
  })

  switch (ack.result) {
    case 'applied':
    case 'deduped':
      return { http: 200, payload: { result: ack.result, fired_at_frame: ack.fired_at_frame ?? null,
                                     at_utc: ack.at_utc ?? entry.at_utc, seq: entry.seq } }
    case 'unknown_node':
      // Distinct from offline on purpose. "It is offline, everything is fine"
      // is a reassuring sentence, and saying it about a node that does not
      // exist would send an operator looking at the wrong machine.
      return { http: 404, payload: { error: 'no_such_node',
        label: `There is no node called “${body.node}”. Check the name in Configure.` } }
    case 'offline':
      return { http: 503, payload: { error: 'node_offline',
        label: `${body.node} is offline. Anything already on air is still on air.` } }
    case 'not_acked':
      return { http: 503, payload: { error: 'cue_not_acked',
        label: `${body.node} didn’t confirm the take. Check the layer before taking again.` } }
    default:
      return { http: 409, payload: { result: 'rejected', error: ack.reason ?? 'rejected',
        label: rejectLabel(ack.reason, body) } }
  }
}

function rejectLabel(reason, body) {
  switch (reason) {
    case 'DeadlinePassed': return 'The take was too late to be safe, so it wasn’t fired.'
    case 'NotReady': return 'That item isn’t staged on the node yet.'
    case 'NoSuchLayer': return `${body.node} has no layer called “${body.layer}”.`
    case 'UnreadableTemplate': return 'That node is too old to read this template.'
    case 'Unsupported': return 'That node doesn’t support scheduled takes.'
    default: return 'The node refused that cue.'
  }
}

const context = (req) => ({
  user: req.get('x-prodmesh-user') ?? null,
  station: req.get('x-prodmesh-station') ?? null,
})

router.post('/api/cue', async (req, res) => {
  const { http, payload } = await dispatch(req.body ?? {}, context(req))
  res.status(http).json(payload)
})

/**
 * `POST /api/cue/batch` — applied in order, for a multi-layer look.
 *
 * In order, and **not in parallel**: a look where the background arrives after
 * the text is a visible flash, and the ordering is the only thing preventing it.
 */
router.post('/api/cue/batch', async (req, res) => {
  const cues = Array.isArray(req.body?.cues) ? req.body.cues : []
  if (!cues.length) {
    return fail(res, 422, 'validation_failed', 'A batch needs at least one cue.')
  }
  const ctx = context(req)
  const results = []
  for (const c of cues) {
    const r = await dispatch({ ...c, show: req.body.show }, ctx)
    results.push({ cue_id: c.cue_id, http: r.http, ...r.payload })
    // Stop at the first genuine failure. Continuing would put half a look on
    // air, which is harder to recover from than none of it.
    if (r.http >= 500 || r.http === 409) break
  }
  const worst = Math.max(...results.map((r) => r.http))
  res.status(worst).json({ results })
})
