/**
 * Screens — the places graphics go (ADR 0023).
 *
 * The first thing in Configure that is about the ROOM rather than about a
 * machine: "LED Wall" and "Lower Third Key", configured once, so that moving a
 * wall to another node is one edit instead of one per Item.
 */

import express from 'express'
import { handle } from '../db.js'
import * as screens from '../screens.js'
import { reofferAll } from '../nodeLink.js'
import { requirePermission } from '../auth.js'
import { newId } from '../ids.js'

export const router = express.Router()

// Screens are plant configuration, so they sit behind the same permission the
// rest of Configure does.
const mayConfigure = requirePermission('graphics.nodes.configure')

const fail = (res, status, error, label, extra = {}) =>
  res.status(status).json({ error, label, ...extra })

router.get('/api/screens', (_req, res) => {
  res.json({ screens: screens.all() })
})

router.post('/api/screens', mayConfigure, (req, res) => {
  const b = req.body ?? {}
  const name = String(b.name ?? '').trim()
  if (!name) return fail(res, 422, 'validation_failed', 'A screen needs a name.')
  const output = String(b.output ?? '').trim()
  const layer = String(b.layer ?? '').trim()
  if (!output || !layer) {
    return fail(res, 422, 'validation_failed', 'A screen needs an output and a layer.')
  }

  const db = handle()
  const id = b.id ?? newId()
  const now = Date.now()
  try {
    db.prepare(
      `INSERT INTO screens (id, name, node_id, output, layer, template_id, created_at, updated_at)
       VALUES (?, ?, ?, ?, ?, ?, ?, ?)`,
    ).run(id, name, b.node_id ?? null, output, layer, b.template_id ?? null, now, now)
  } catch (e) {
    // The name is unique because it is what an operator picks from a list, and
    // two places called "LED Wall" is a choice nobody can make correctly.
    if (String(e.message).includes('UNIQUE')) {
      return fail(res, 409, 'already_exists', `There is already a screen called “${name}”.`)
    }
    throw e
  }
  res.status(201).json(screens.get(id))
})

router.patch('/api/screens/:id', mayConfigure, (req, res) => {
  const existing = screens.get(req.params.id)
  if (!existing) return fail(res, 404, 'not_found', 'There is no screen with that id.')

  const b = req.body ?? {}
  const next = {
    name: b.name != null ? String(b.name).trim() : existing.name,
    node_id: 'node_id' in b ? (b.node_id ?? null) : existing.node_id,
    output: b.output != null ? String(b.output).trim() : existing.output,
    layer: b.layer != null ? String(b.layer).trim() : existing.layer,
    template_id: 'template_id' in b ? (b.template_id ?? null) : existing.template_id,
  }
  if (!next.name || !next.output || !next.layer) {
    return fail(res, 422, 'validation_failed', 'A screen needs a name, an output, and a layer.')
  }

  handle()
    .prepare(
      `UPDATE screens SET name = ?, node_id = ?, output = ?, layer = ?, template_id = ?,
              updated_at = ? WHERE id = ?`,
    )
    .run(next.name, next.node_id, next.output, next.layer, next.template_id, Date.now(), req.params.id)

  // Every Item pointing here now resolves somewhere else, so the nodes need
  // telling. Items themselves are untouched — that is the whole point.
  reofferAll()
  res.json(screens.get(req.params.id))
})

/**
 * Delete a screen.
 *
 * **Refused while Items point at it**, and the Items are NAMED — the same rule
 * the fields editor follows, for the same reason: the alternative is
 * destinations that silently render nowhere, discovered by a graphic not
 * appearing on a Sunday.
 */
router.delete('/api/screens/:id', mayConfigure, (req, res) => {
  const existing = screens.get(req.params.id)
  if (!existing) return fail(res, 404, 'not_found', 'There is no screen with that id.')

  const users = screens.usedBy(req.params.id)
  if (users.length) {
    const shown = users.slice(0, 5).join(', ')
    const more = users.length > 5 ? `, and ${users.length - 5} more` : ''
    return fail(
      res,
      409,
      'in_use',
      // The verb agrees too. "1 Item go" is the kind of thing that makes an
      // operator trust the rest of the sentence less.
      users.length === 1
        ? `“${existing.name}” is where ${shown} goes. Point it somewhere else first.`
        : `“${existing.name}” is where ${users.length} Items go: ${shown}${more}. Point them somewhere else first.`,
      { items: users },
    )
  }

  handle().prepare('DELETE FROM screens WHERE id = ?').run(req.params.id)
  res.status(204).end()
})
