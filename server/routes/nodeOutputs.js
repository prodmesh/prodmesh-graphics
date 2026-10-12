/**
 * A node's output HARDWARE (ADR 0024).
 *
 * The node owns these and persists them itself; this is the server's record of
 * what an operator last asked for, so Configure has something to show before a
 * node connects and something to re-send when one does. If the two ever
 * disagree, the node is right — it is the thing with the wire in the back of
 * it — and its state report says so.
 */

import express from 'express'
import { handle } from '../db.js'
import * as link from '../nodeLink.js'
import { requirePermission } from '../auth.js'

export const router = express.Router()

const mayConfigure = requirePermission('graphics.nodes.configure')
const fail = (res, status, error, label, extra = {}) =>
  res.status(status).json({ error, label, ...extra })

const BACKENDS = new Set(['ndi', 'decklink', 'window', 'headless'])

/** What this server last asked a node for, in order. */
export function desiredOutputs(nodeId) {
  return handle()
    .prepare('SELECT config FROM node_outputs WHERE node_id = ? ORDER BY ordinal')
    .all(nodeId)
    .map((r) => JSON.parse(r.config))
}

router.get('/api/nodes/:id/output-config', (req, res) => {
  res.json({
    outputs: desiredOutputs(req.params.id),
    // What the node did with it, when it has said. Null before it has.
    ack: link.outputsAck(req.params.id),
  })
})

/**
 * Replace a node's output hardware.
 *
 * `force` reconfigures an output that has something on air. Without it the node
 * refuses that output by name and keeps it running — closing a device takes the
 * graphic with it, and an operator who means that should have to say so.
 */
router.put('/api/nodes/:id/output-config', mayConfigure, (req, res) => {
  const list = req.body?.outputs
  if (!Array.isArray(list) || list.length === 0) {
    return fail(res, 422, 'validation_failed', 'A node needs at least one output.')
  }

  const seen = new Set()
  const outputs = []
  for (const o of list) {
    const id = String(o?.id ?? '').trim()
    if (!id) return fail(res, 422, 'validation_failed', 'Every output needs an id.')
    if (seen.has(id)) {
      // Two outputs with one id makes "take to wall" ambiguous, and the node
      // would resolve it to whichever came first.
      return fail(res, 422, 'validation_failed', `Two outputs share the id “${id}”.`)
    }
    seen.add(id)
    if (!BACKENDS.has(o.backend)) {
      // Named rather than coerced to a default: an output that quietly became
      // NDI when someone meant DeckLink is a silent failure on a Sunday.
      return fail(res, 422, 'validation_failed', `“${o.backend}” is not a backend this server knows.`)
    }
    if (o.backend === 'decklink' && o.key_device != null && o.key_device === o.device) {
      return fail(
        res,
        422,
        'validation_failed',
        'The key half of a fill+key pair has to be a different port from the fill.',
      )
    }
    // Refused, not dropped: someone who ticked it for an NDI feed believes
    // their matrix and ranges are applied, and they would not be — NDI carries
    // RGB and its receiver converts (ADR 0012).
    if (o.ycbcr === true && o.backend !== 'decklink') {
      return fail(
        res,
        422,
        'validation_failed',
        `Output “${id}” can’t encode YCbCr: only an SDI output carries it. ${String(o.backend).toUpperCase()} sends RGB and the receiver converts.`,
      )
    }
    outputs.push({
      id,
      backend: o.backend,
      ndi_name: String(o.ndi_name ?? ''),
      device: Number(o.device ?? 0),
      key_device: o.key_device == null ? null : Number(o.key_device),
      mode: String(o.mode ?? ''),
      width: Number(o.width ?? 1920),
      height: Number(o.height ?? 1080),
      fps_num: Number(o.fps_num ?? 60000),
      fps_den: Number(o.fps_den ?? 1001),
      audio_channels: Number(o.audio_channels ?? 0),
      // WHERE the audio goes (ADR 0028). Empty is embedded in the video, which
      // is what every output meant before this existed. The name is passed
      // through rather than checked against a device list: the node is the only
      // thing that knows what is plugged into it, and it refuses by name.
      audio_device: String(o.audio_device ?? ''),
      audio_host: String(o.audio_host ?? ''),
      preview_port: Number(o.preview_port ?? 0),
      group: String(o.group ?? 'default'),
      // The node encodes v210 itself, applying the colour settings, instead
      // of handing the card RGB to convert with its own.
      ycbcr: o.ycbcr === true,
    })
  }

  const db = handle()
  const now = Date.now()
  db.transaction(() => {
    db.prepare('DELETE FROM node_outputs WHERE node_id = ?').run(req.params.id)
    const ins = db.prepare(
      `INSERT INTO node_outputs (node_id, output_id, config, ordinal, updated_at)
       VALUES (?, ?, ?, ?, ?)`,
    )
    outputs.forEach((o, i) => ins.run(req.params.id, o.id, JSON.stringify(o), i, now))
  })()

  const sent = link.setOutputs(req.params.id, outputs, { force: req.body?.force === true })
  res.json({
    outputs,
    // An offline node is not a failure — the configuration is stored and
    // re-sent when it connects. Saying which happened is the honest part.
    delivered: !!sent?.sent,
    gen: sent?.gen ?? null,
  })
})
