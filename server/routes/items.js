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
import { requirePermission } from '../auth.js'

const mayEdit = requirePermission('graphics.rundown.edit')

export const router = express.Router()

const fail = (res, status, error, label, detail) =>
  res.status(status).json({ error, label, ...(detail ? { detail } : {}) })

const hydrate = (r, db) => ({
  ...r,
  data: JSON.parse(r.data),
  // Kept for callers written against one destination. It is the FIRST
  // destination, not the only one, and `destinations` is the truth.
  destination: JSON.parse(r.destination),
  destinations: db ? destinationsOf(db, r.id) : [],
})

function revision(db, rev) {
  const r = db.prepare('SELECT * FROM template_revisions WHERE template_rev = ?').get(rev)
  return r ? { ...r, manifest: JSON.parse(r.manifest) } : null
}

/**
 * The destinations an Item goes to (ADR 0018).
 *
 * One is the common case and stays a one-line answer. Two or three is the norm
 * in a real service: the same words to a lower-third key, a full-screen version,
 * and an LED wall, each with its own template.
 */
function destinationsOf(db, itemId) {
  return db
    .prepare(
      `SELECT ordinal, node_id, output, layer, template_rev, mode, blank
         FROM item_destinations WHERE item_id = ? ORDER BY ordinal`,
    )
    .all(itemId)
    .map((d) => ({ ...d, blank: !!d.blank }))
}

/**
 * Normalise whatever the caller sent into a destination list.
 *
 * Accepts the old single-destination shape so existing callers keep working,
 * because an API that breaks on the day the model grows teaches people to pin
 * versions.
 */
function normaliseDestinations(body, fallbackRev) {
  const list = Array.isArray(body.destinations)
    ? body.destinations
    : [{ ...body.destination, template_rev: body.template_rev }]

  return list.map((d, i) => ({
    ordinal: i,
    node_id: d.node_id ?? d.node ?? null,
    output: String(d.output ?? 'out-0'),
    layer: String(d.layer ?? 'Lower Third'),
    template_rev: d.template_rev ?? fallbackRev,
    mode: d.mode === 'on_request' ? 'on_request' : 'auto',
    // A blank destination renders nothing HERE, deliberately — clearing that
    // feed is part of what taking this Item means.
    blank: d.blank ? 1 : 0,
  }))
}

/**
 * Every destination's template must accept this Item's data.
 *
 * Checked at save, not at take: two templates rendering the same Item have to
 * agree about its fields, and a destination that needs a field the Item does
 * not carry is a mistake worth naming at the point it was made.
 *
 * A **blank** destination is exempt. It renders nothing, so it demands nothing.
 */
function validateDestinations(db, dests, data) {
  const problems = []
  for (const d of dests) {
    if (d.blank) continue
    const r = revision(db, d.template_rev)
    if (!r) {
      problems.push(`Destination ${d.output} · ${d.layer} names a template revision this server does not have.`)
      continue
    }
    for (const p of validateItemData(r.manifest, data, { hasAsset: cas.has })) {
      problems.push(`${d.output} · ${d.layer}: ${p}`)
    }
  }
  return problems
}

function writeDestinations(db, itemId, dests) {
  db.prepare('DELETE FROM item_destinations WHERE item_id = ?').run(itemId)
  const ins = db.prepare(
    `INSERT INTO item_destinations (item_id, ordinal, node_id, output, layer, template_rev, mode, blank)
     VALUES (?, ?, ?, ?, ?, ?, ?, ?)`,
  )
  for (const d of dests) {
    ins.run(itemId, d.ordinal, d.node_id, d.output, d.layer, d.template_rev, d.mode, d.blank)
  }
}

router.get('/api/items', (req, res) => {
  const db = handle()
  const rows = req.query.template
    ? db.prepare('SELECT * FROM items WHERE template_id = ? ORDER BY id').all(req.query.template)
    : db.prepare('SELECT * FROM items ORDER BY id').all()
  res.json({ items: rows.map((r) => hydrate(r, db)) })
})

router.get('/api/items/:id', (req, res) => {
  const db = handle()
  const r = db.prepare('SELECT * FROM items WHERE id = ?').get(req.params.id)
  if (!r) return fail(res, 404, 'not_found', 'There is no item with that id.')
  res.json(hydrate(r, db))
})

router.post('/api/items', mayEdit, (req, res) => {
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

  const data = body.data ?? {}
  const dests = normaliseDestinations(body, rev)
  // Every destination's template, not just the Item's own — an Item that
  // renders on the key feed and not on the wall is a mistake made now and
  // discovered on Sunday.
  const problems = validateDestinations(db, dests, data)
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
    data: JSON.stringify(data),
    destination: JSON.stringify(body.destination ?? { output: dests[0].output, layer: dests[0].layer }),
    created_at: now,
    updated_at: now,
  }
  db.prepare(
    `INSERT INTO items (id, take_id, label, template_id, template_rev, data, destination, created_at, updated_at)
     VALUES (@id, @take_id, @label, @template_id, @template_rev, @data, @destination, @created_at, @updated_at)`,
  ).run(row)
  writeDestinations(db, id, dests)
  res.status(201).json(hydrate(row, db))
})

router.patch('/api/items/:id', mayEdit, (req, res) => {
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
  // Destinations are replaced wholesale when given, and left alone when not —
  // a PATCH that only changes a title must not silently drop the wall feed.
  const dests =
    body.destinations || body.destination || body.template_rev
      ? normaliseDestinations(body, rev)
      : destinationsOf(db, cur.id).map((d) => ({ ...d, blank: d.blank ? 1 : 0 }))

  // Re-validated against whichever revisions this Item will END UP on. Moving
  // to a revision that dropped a field it fills must fail here, loudly, rather
  // than quietly render without it.
  const problems = validateDestinations(db, dests, data)
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
  writeDestinations(db, cur.id, dests)
  res.json(hydrate(db.prepare('SELECT * FROM items WHERE id = ?').get(cur.id), db))
})

router.delete('/api/items/:id', mayEdit, (req, res) => {
  const info = handle().prepare('DELETE FROM items WHERE id = ?').run(req.params.id)
  if (!info.changes) return fail(res, 404, 'not_found', 'There is no item with that id.')
  res.status(204).end()
})
