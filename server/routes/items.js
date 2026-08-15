/**
 * Items — data bound to a template revision, aimed at a destination.
 *
 * # The pinning rule
 *
 * An Item stores `template_rev`, not `template_id` alone. It is **pinned to a
 * revision and does not follow head.** Publishing a new revision on Thursday
 * must not change what Sunday's rundown renders; moving an Item forward is a
 * deliberate act (`PATCH` with a new `template_rev`), and the response says what
 * it would break before it breaks it.
 *
 * # Validation happens here, at save
 *
 * Data is checked against the pinned revision's field schema when it is saved —
 * not when it is taken. An Item that cannot render should say so while someone
 * is looking at it, which is the same instinct as refusing an unreadable
 * template at assignment time rather than at take time.
 *
 * Spec: `docs/specs/http-api.md` §3.
 */

import express from 'express'
import * as cas from '../cas.js'
import { handle } from '../db.js'
import { validateItemData } from '../manifest.js'
import { newId } from '../ids.js'

export const router = express.Router()

const fail = (res, status, error, label, detail) =>
  res.status(status).json({ error, label, ...(detail ? { detail } : {}) })

const hydrate = (r) => ({
  ...r,
  data: JSON.parse(r.data),
  destination: JSON.parse(r.destination),
})

function revision(db, rev) {
  const r = db.prepare('SELECT * FROM template_revisions WHERE template_rev = ?').get(rev)
  return r ? { ...r, manifest: JSON.parse(r.manifest) } : null
}

router.get('/api/items', (req, res) => {
  const db = handle()
  const rows = req.query.template
    ? db.prepare('SELECT * FROM items WHERE template_id = ? ORDER BY id').all(req.query.template)
    : db.prepare('SELECT * FROM items ORDER BY id').all()
  res.json({ items: rows.map(hydrate) })
})

router.get('/api/items/:id', (req, res) => {
  const r = handle().prepare('SELECT * FROM items WHERE id = ?').get(req.params.id)
  if (!r) return fail(res, 404, 'not_found', 'There is no item with that id.')
  res.json(hydrate(r))
})

router.post('/api/items', (req, res) => {
  const db = handle()
  const body = req.body ?? {}

  // Default to head, because an operator creating an Item today means "the
  // template as it is now" — but store the resolved revision, so it is pinned
  // from the moment it exists rather than at some later first-take.
  let rev = body.template_rev
  if (!rev && body.template_id) {
    rev = db.prepare('SELECT head_rev FROM templates WHERE id = ?').get(body.template_id)?.head_rev
  }
  if (!rev) {
    return fail(res, 422, 'validation_failed', 'Choose a template that has been published at least once.')
  }

  const r = revision(db, rev)
  if (!r) return fail(res, 404, 'not_found', 'That template revision is not on this server.')

  const problems = validateItemData(r.manifest, body.data ?? {}, { hasAsset: cas.has })
  if (problems.length) {
    return fail(res, 422, 'validation_failed', problems[0], { problems })
  }

  const id = body.id ?? newId()
  const now = Date.now()
  const row = {
    id,
    take_id: body.take_id ?? null,
    label: String(body.label ?? '').trim() || r.name,
    template_id: r.template_id,
    template_rev: rev,
    data: JSON.stringify(body.data ?? {}),
    destination: JSON.stringify(body.destination ?? {}),
    created_at: now,
    updated_at: now,
  }
  db.prepare(
    `INSERT INTO items (id, take_id, label, template_id, template_rev, data, destination, created_at, updated_at)
     VALUES (@id, @take_id, @label, @template_id, @template_rev, @data, @destination, @created_at, @updated_at)`,
  ).run(row)
  res.status(201).json(hydrate(row))
})

router.patch('/api/items/:id', (req, res) => {
  const db = handle()
  const cur = db.prepare('SELECT * FROM items WHERE id = ?').get(req.params.id)
  if (!cur) return fail(res, 404, 'not_found', 'There is no item with that id.')

  const body = req.body ?? {}
  const rev = body.template_rev ?? cur.template_rev
  const r = revision(db, rev)
  if (!r) return fail(res, 404, 'not_found', 'That template revision is not on this server.')
  if (r.template_id !== cur.template_id) {
    return fail(res, 422, 'validation_failed', 'That revision belongs to a different template.')
  }

  const data = body.data ?? JSON.parse(cur.data)
  // Re-validated against whichever revision this Item will END UP on. Moving an
  // Item to a revision that dropped a field it fills must fail here, loudly,
  // rather than quietly render without it.
  const problems = validateItemData(r.manifest, data, { hasAsset: cas.has })
  if (problems.length) {
    return fail(res, 422, 'validation_failed', problems[0], { problems })
  }

  const now = Date.now()
  db.prepare(
    `UPDATE items SET take_id = ?, label = ?, template_rev = ?, data = ?, destination = ?, updated_at = ?
      WHERE id = ?`,
  ).run(
    body.take_id !== undefined ? body.take_id : cur.take_id,
    body.label !== undefined ? String(body.label).trim() : cur.label,
    rev,
    JSON.stringify(data),
    JSON.stringify(body.destination ?? JSON.parse(cur.destination)),
    now,
    cur.id,
  )
  res.json(hydrate(db.prepare('SELECT * FROM items WHERE id = ?').get(cur.id)))
})

router.delete('/api/items/:id', (req, res) => {
  const info = handle().prepare('DELETE FROM items WHERE id = ?').run(req.params.id)
  if (!info.changes) return fail(res, 404, 'not_found', 'There is no item with that id.')
  res.status(204).end()
})
