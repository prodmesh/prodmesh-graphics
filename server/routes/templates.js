/**
 * Templates and their revisions.
 *
 * A Template is a name and an identity. A **Template Revision** is the thing
 * that actually renders, and it is immutable and content-addressed:
 * `template_rev = blake3(canonical CBOR of the SceneDoc)`.
 *
 * Publishing is therefore not an update. It writes a new revision and moves
 * `head_rev`; existing Items keep pointing at whatever they were pinned to.
 * That pinning is what stops a Thursday edit from changing Sunday's rundown
 * under an operator who never touched it.
 *
 * Spec: `docs/specs/http-api.md` §3.
 */

import express from 'express'
import * as cas from '../cas.js'
import { handle } from '../db.js'
import { validateManifest } from '../manifest.js'
import { newId } from '../ids.js'
import { requirePermission } from '../auth.js'

const mayDesign = requirePermission('graphics.templates.edit')

export const router = express.Router()

const fail = (res, status, error, label, detail) =>
  res.status(status).json({ error, label, ...(detail ? { detail } : {}) })

router.get('/api/templates', (_req, res) => {
  const rows = handle()
    .prepare(
      `SELECT t.id, t.name, t.created_at, t.updated_at, t.head_rev,
              (SELECT COUNT(*) FROM template_revisions r WHERE r.template_id = t.id) AS revisions
         FROM templates t ORDER BY t.updated_at DESC`,
    )
    .all()
  res.json({ templates: rows })
})

router.get('/api/templates/:id', (req, res) => {
  const db = handle()
  const t = db.prepare('SELECT * FROM templates WHERE id = ?').get(req.params.id)
  if (!t) return fail(res, 404, 'not_found', 'There is no template with that id.')

  const head = t.head_rev
    ? db.prepare('SELECT * FROM template_revisions WHERE template_rev = ?').get(t.head_rev)
    : null

  res.json({ ...t, head: head ? { ...head, manifest: JSON.parse(head.manifest) } : null })
})

router.get('/api/templates/:id/revisions', (req, res) => {
  const db = handle()
  const pinned = db.prepare(
    `SELECT DISTINCT id, label FROM items
      WHERE template_rev = @rev
         OR id IN (SELECT item_id FROM item_destinations WHERE template_rev = @rev)`,
  )
  const rows = db
    .prepare(
      `SELECT template_rev, name, min_reader, manifest, published_at, published_by
         FROM template_revisions WHERE template_id = ? ORDER BY published_at DESC`,
    )
    .all(req.params.id)
    // The manifest rides along so a client can DIFF two revisions — fields,
    // timelines, assets — without the server growing a diff format of its
    // own. Items pinned here, by their own rev or by any destination's, are
    // what "update Items" would move.
    .map((r) => ({
      ...r,
      manifest: JSON.parse(r.manifest),
      items: pinned.all({ rev: r.template_rev }),
    }))
  res.json({ revisions: rows })
})

router.post('/api/templates', mayDesign, (req, res) => {
  const name = String(req.body?.name ?? '').trim()
  if (!name) return fail(res, 422, 'validation_failed', 'A template needs a name.')

  const db = handle()
  const id = req.body?.id ?? newId()

  // A caller-supplied id that already exists is a conflict, not a crash. It
  // happens for real: Design retries a create after a flaky response, and the
  // second attempt must not produce a 500 that hides an otherwise-fine state.
  const existing = db.prepare('SELECT * FROM templates WHERE id = ?').get(id)
  if (existing) {
    return fail(res, 409, 'already_exists', 'A template with that id is already here.')
  }

  const now = Date.now()
  db.prepare('INSERT INTO templates (id, name, created_at, updated_at) VALUES (?, ?, ?, ?)')
    .run(id, name, now, now)
  res.status(201).json({ id, name, created_at: now, updated_at: now, head_rev: null })
})

/**
 * Publish a compiled SceneDoc.
 *
 * Body: `{ scene: <base64 canonical CBOR>, manifest: {...} }`
 *
 * The bytes go into the CAS **first**, and the address they land at is checked
 * against the manifest's `template_rev`. Doing it in that order means the server
 * never has to take the client's word for what it just published — and since
 * revisions are immutable, a revision whose id did not match its bytes would be
 * a permanent lie rather than a recoverable mistake.
 */
router.post('/api/templates/:id/publish', mayDesign, async (req, res) => {
  const db = handle()
  const t = db.prepare('SELECT * FROM templates WHERE id = ?').get(req.params.id)
  if (!t) return fail(res, 404, 'not_found', 'There is no template with that id.')

  const b64 = req.body?.scene
  if (typeof b64 !== 'string' || b64.length === 0) {
    return fail(res, 422, 'validation_failed', 'The publish is missing its compiled scene.')
  }

  // The JSON mirror, if the client sent one. Stored by address like anything
  // else, and never parsed here.
  let mirrorHash = null
  if (typeof req.body?.mirror === 'string' && req.body.mirror.length) {
    mirrorHash = (await cas.put(Buffer.from(req.body.mirror, 'utf8'))).hash
  }

  let stored
  try {
    stored = await cas.put(Buffer.from(b64, 'base64'))
  } catch (e) {
    return fail(res, 422, 'validation_failed', 'The compiled scene could not be stored.', e.message)
  }

  let manifest
  try {
    manifest = validateManifest(req.body?.manifest, stored.hash)
  } catch (e) {
    // The blob stays. It is content-addressed, so it is inert until something
    // references it, and GC is a separate deliberate operator action.
    return fail(res, 422, 'validation_failed', 'This template did not compile to something publishable.', e.detail ?? e.message)
  }

  if (manifest.template_id !== t.id) {
    return fail(res, 422, 'validation_failed', 'That scene belongs to a different template.')
  }

  const now = Date.now()
  const existing = db
    .prepare('SELECT template_rev FROM template_revisions WHERE template_rev = ?')
    .get(manifest.template_rev)

  if (!existing) {
    db.transaction(() => {
      db.prepare(
        `INSERT INTO template_revisions
           (template_rev, template_id, name, min_reader, manifest, published_at, published_by, mirror_hash)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)`,
      ).run(
        manifest.template_rev,
        t.id,
        manifest.name,
        manifest.min_reader,
        JSON.stringify(manifest),
        now,
        req.body?.published_by ?? null,
        mirrorHash,
      )
      db.prepare('UPDATE templates SET head_rev = ?, name = ?, updated_at = ? WHERE id = ?')
        .run(manifest.template_rev, manifest.name, now, t.id)
    })()
  } else {
    // Republishing identical bytes is a no-op on the revision and still moves
    // head — that is what "publish this again" should mean, and it is only
    // safe to treat as idempotent BECAUSE the id is the hash.
    db.prepare('UPDATE templates SET head_rev = ?, updated_at = ? WHERE id = ?')
      .run(manifest.template_rev, now, t.id)
    // A mirror can arrive with a republish of bytes we already had — for
    // instance the first publish that came from `pmgctl`, which does not send
    // one. Backfilling it costs nothing and makes the template openable.
    if (mirrorHash) {
      db.prepare('UPDATE template_revisions SET mirror_hash = COALESCE(mirror_hash, ?) WHERE template_rev = ?')
        .run(mirrorHash, manifest.template_rev)
    }
  }

  res.status(existing ? 200 : 201).json({
    template_rev: manifest.template_rev,
    template_id: t.id,
    deduped: Boolean(existing) || stored.deduped,
    mirror_hash: mirrorHash,
    warnings: manifest.warnings ?? [],
    // Assets the template needs that this server does not hold. Design should
    // say so plainly rather than let a node discover it at staging time.
    missing_assets: (manifest.assets ?? []).filter((h) => !cas.has(h)),
  })
})
