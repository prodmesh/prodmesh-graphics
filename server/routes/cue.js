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
import * as screens from '../screens.js'
import * as link from '../nodeLink.js'
import { append } from '../cueLog.js'
import { identify, requirePermission } from '../auth.js'

/** A trusted station's scope is checked against the output and layer it names. */
export const cueScope = (body) => ({ output: body?.output, layer: body?.layer })

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
/**
 * The destinations of an Item that a given node can serve.
 *
 * Filtered the same way the assignment is, and for the same reason: a cue
 * naming an output a node does not have is a cue it must refuse, and sending
 * one is asking a question we already know the answer to.
 *
 * Returns `[]` for an Item created before destinations existed, which makes the
 * caller fall back to the single output/layer it was given.
 */
function destinationsFor(itemId, nodeId) {
  const outs = new Set(
    handle()
      .prepare('SELECT output_id FROM outputs WHERE node_id = ?')
      .all(nodeId ?? '')
      .map((o) => o.output_id),
  )
  const rows = handle()
    .prepare(
      `SELECT node_id, output, layer, template_rev, mode, blank, screen_id
         FROM item_destinations WHERE item_id = ? ORDER BY ordinal`,
    )
    .all(itemId)
  // Through the screen, if it names one — the same resolution the assignment
  // build does, from the same function, because staging a graphic for one place
  // and firing it at another is the failure two copies of this would produce.
  return screens
    .resolveAll(rows)
    .filter((d) => !d.node_id || !nodeId || d.node_id === nodeId)
    // An unconfigured node has no rows in `outputs` yet and still has to work,
    // so an empty set means "cannot rule anything out" rather than "nothing".
    .filter((d) => outs.size === 0 || outs.has(d.output))
    .map((d) => ({
      output: d.output,
      layer: d.layer,
      template_rev: d.template_rev,
      mode: d.mode,
      blank: !!d.blank,
    }))
}

/**
 * Which nodes serve any destination of this Item.
 *
 * A destination naming no node belongs to whoever has that output, so with one
 * node this is simply that node — which is the honest answer before Configure
 * can name them.
 */
function nodesFor(itemId, explicit) {
  if (explicit) return [explicit]
  const db = handle()
  const dests = db
    .prepare('SELECT node_id, output FROM item_destinations WHERE item_id = ?')
    .all(itemId)
  if (!dests.length) return []

  const named = [...new Set(dests.map((d) => d.node_id).filter(Boolean))]
  const unnamed = dests.filter((d) => !d.node_id).map((d) => d.output)

  // An unnamed destination goes to whichever adopted node actually has that
  // output. Asking the outputs table rather than guessing is what stops a
  // take being sent to a node that would only refuse it.
  const byOutput = unnamed.length
    ? db
        .prepare(
          `SELECT DISTINCT node_id FROM outputs
            WHERE output_id IN (${unnamed.map(() => '?').join(',')})`,
        )
        .all(...unnamed)
        .map((r) => r.node_id)
    : []

  return [...new Set([...named, ...byOutput])]
}

/** Why one destination did not fire, as the end of a sentence. */
function whyNot(a) {
  switch (a.result) {
    case 'offline':      return 'it is offline'
    case 'unknown_node': return 'there is no node by that name'
    case 'superseded':   return 'a newer take replaced it before it fired'
    case 'not_adopted':  return 'it has not been adopted yet'
    case 'not_acked':    return 'it did not confirm in time'
    // rejectLabel() would be tempting here and is wrong: it names the node
    // itself, and this clause already sits after one.
    default:             return a.detail ?? `it refused the cue (${a.reason ?? 'rejected'})`
  }
}

/**
 * One sentence describing a partly-delivered take.
 *
 * Says what IS on air first. That is the thing an operator needs before they
 * decide whether to press anything again, and burying it behind the failure is
 * how someone ends up taking a graphic twice on the node that already had it.
 */
function partialLabel(acks) {
  const ok = acks.filter((a) => a.result === 'applied' || a.result === 'deduped')
  const bad = acks.filter((a) => !ok.includes(a))
  const on = ok.length
    ? `On air on ${ok.map((a) => a.node).join(' and ')}.`
    : 'Not on air anywhere.'
  const off = bad.map((a) => `${a.node} did not confirm: ${whyNot(a)}.`).join(' ')
  return `${on} ${off}`.trim()
}

function buildOp(body, item) {
  const output = body.output
  const layer = body.layer
  switch (body.op) {
    // A take fans out to every AUTO destination this node serves (ADR 0018).
    // One cue per node carrying that node's destinations, because two cues to
    // one node sharing a cue_id would dedupe each other — which is exactly
    // what a lower third plus its full-screen version on one card would be.
    case 'take': {
      const dests = destinationsFor(item.id, body.node)
      const to = { output: dests[0]?.output ?? output, layer: dests[0]?.layer ?? layer }
      if (!to.output || !to.layer) {
        // Never build a cue with a hole in it. CBOR encodes a missing JS field
        // as `undefined`, which the node cannot decode AT ALL — so a take whose
        // destinations resolved to nothing arrived as an undecodable frame and
        // came back as "undefined didn’t confirm the take". The Item has
        // somewhere to go or it does not, and saying which is the whole job.
        return [null, 'no_destination']
      }
      return [
        {
          Take: {
            item: item.id,
            output: to.output,
            layer: to.layer,
            destinations: dests,
          },
        },
        null,
      ]
    }
    // "Also put it full screen" — fires the destinations a take skipped
    // (ADR 0018). The server sends the SAME destination list; the node picks
    // the on_request ones, so the two ops cannot disagree about what an Item's
    // destinations are.
    case 'take_also': {
      const dests = destinationsFor(item.id, body.node)
      const wanted = dests.filter((d) => d.mode === 'on_request')
      if (!wanted.length) {
        return [null, 'Nothing on this item waits to be asked for.']
      }
      return [
        {
          TakeAlso: {
            item: item.id,
            output: wanted[0].output,
            layer: wanted[0].layer,
            destinations: dests,
          },
        },
        null,
      ]
    }
    case 'arm':       return [{ Arm: { item: item.id, output, layer } }, null]
    case 'take_out':  return [{ TakeOut: { output, layer } }, null]
    // tween_ms rides through untouched. 0 is a hard cut — correcting a
    // misspelled name should not be a 400 ms animation about itself.
    case 'update':    return [{ Update: { output, layer, data: toWireData(body.data ?? {}),
                                          tween_ms: body.tween_ms ?? null } }, null]
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

const OPS_NEEDING_ITEM = new Set(['take_also', 'take', 'arm', 'preload'])

export async function dispatch(body, ctx) {
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

  // Answerable here, without asking a node.
  //
  // Since the fan-out, a problem from buildOp surfaces as a per-node rejection
  // rather than a validation failure — right for something only the node can
  // judge, wrong for this: the server holds the destinations, so a round trip
  // to be told nothing happened is worse than a sentence.
  if (body.op === 'take_also') {
    const any = handle()
      .prepare(`SELECT 1 FROM item_destinations WHERE item_id = ? AND mode = 'on_request'`)
      .get(item.id)
    if (!any) {
      return {
        http: 422,
        payload: {
          error: 'nothing_on_request',
          label: 'Nothing on this item waits to be asked for.',
        },
      }
    }
  }

  // Which nodes this action reaches. One for everything except a take, which
  // an Item's destinations may spread across several (ADR 0018).
  const targets = ['take', 'take_also'].includes(body.op)
    ? nodesFor(item.id, body.node)
    : [body.node]
  if (!targets.length) {
    return {
      http: 409,
      payload: {
        error: 'no_destination',
        label: 'That item has no destination on any connected node.',
      },
    }
  }

  // Fanned in parallel, sharing one cue_id. Safe because dedupe is scoped per
  // node, and RIGHT because the operator pressed one button: two ids would
  // make one action look like two in the log.
  const acks = await Promise.all(
    targets.map(async (node) => {
      const [op, problem] = buildOp({ ...body, node }, item)
      if (problem) return { node, result: 'rejected', detail: problem }
      const a = await link.cue(node, {
        cue_id: body.cue_id,
        op,
        deadline_ms: body.deadline_ms,
      })
      return { node, ...a }
    }),
  )

  // The single-node answer, unchanged, so every existing caller and every
  // existing test still sees exactly what it saw.
  const ack = acks[0]
  const fired = acks.filter((a) => a.result === 'applied' || a.result === 'deduped')
  // Superseded is the system working, not failing (ADR 0017): a node that
  // replaced this take with a newer one did what the operator meant. It is not
  // "fired", and it must not turn a fan-out into a partial delivery either.
  const settled = acks.filter((a) => fired.includes(a) || a.result === 'superseded')
  const partial = acks.length > 1 && fired.length > 0 && settled.length < acks.length

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
    // One operator action, one log entry, and the per-destination outcomes
    // beside it. Logging one row per node would make a fan-out look like
    // several takes to whoever reads it afterwards.
    result: partial ? 'partial' : ack.result,
    destinations: acks.length > 1 ? acks.map((a) => ({ node: a.node, result: a.result })) : undefined,
  })

  // A partly-delivered take.
  //
  // Deliberately a FAILURE status carrying success detail, not a 2xx. The take
  // button must not go green unless everything fired, and a client that checks
  // only `res.ok` would show green on any 2xx — including 207. Being wrong in
  // that direction is the failure ADR 0008 exists to prevent, so the safe
  // default wins and richer clients read `destinations` for the specifics.
  if (partial) {
    return {
      http: 503,
      payload: {
        result: 'partial',
        error: 'partly_delivered',
        label: partialLabel(acks),
        destinations: acks.map((a) => ({
          node: a.node,
          result: a.result,
          detail: a.detail ?? a.reason ?? null,
        })),
        seq: entry.seq,
      },
    }
  }

  switch (ack.result) {
    case 'applied':
    case 'deduped':
      return {
        http: 200,
        payload: {
          result: ack.result,
          fired_at_frame: ack.fired_at_frame ?? null,
          at_utc: ack.at_utc ?? entry.at_utc,
          seq: entry.seq,
          ...(acks.length > 1
            ? { destinations: acks.map((a) => ({ node: a.node, result: a.result })) }
            : {}),
        },
      }
    // Not an error and not a success: a newer take for the same layer replaced
    // this one before it fired (ADR 0017). 200, because the system did exactly
    // what it should have — the operator's intent is their latest position, not
    // every slide they passed through on the way. The distinction from a
    // rejected-as-late cue has to survive to here, because one is the system
    // working and the other is the system refusing. It used to fall through to
    // the default below, so every superseded Follow keypress read as "the node
    // refused that cue".
    case 'superseded':
      return { http: 200, payload: { result: 'superseded', seq: entry.seq,
        label: 'Replaced by a newer take before it fired.' } }
    case 'not_adopted':
      return { http: 409, payload: { error: 'not_adopted',
        label: `${body.node} hasn’t been adopted yet. Adopt it in Configure before taking to it.` } }
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
    case 'NoSuchTimeline': return `The graphic on air has no timeline named “${body.timeline}”.`
    default: return 'The node refused that cue.'
  }
}

/**
 * Who to record in the log: the identity the server VERIFIED, never a header
 * the caller wrote. This used to log `x-prodmesh-user` — which anyone can set —
 * as the user, and the raw station TOKEN as the station, so the log was both
 * forgeable and holding a credential.
 */
export const context = (who) => ({
  user: who?.user?.username ?? null,
  station: who?.station?.name ?? null,
})

router.post('/api/cue', requirePermission('graphics.operate', (req) => cueScope(req.body)), async (req, res) => {
  const { http, payload } = await dispatch(req.body ?? {}, context(req.who ?? identify(req)))
  res.status(http).json(payload)
})

/**
 * `POST /api/cue/batch` — applied in order, for a multi-layer look.
 *
 * In order, and **not in parallel**: a look where the background arrives after
 * the text is a visible flash, and the ordering is the only thing preventing it.
 */
router.post('/api/cue/batch', requirePermission('graphics.operate'), async (req, res) => {
  const cues = Array.isArray(req.body?.cues) ? req.body.cues : []
  if (!cues.length) {
    return fail(res, 422, 'validation_failed', 'A batch needs at least one cue.')
  }
  const ctx = context(req.who ?? identify(req))
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
