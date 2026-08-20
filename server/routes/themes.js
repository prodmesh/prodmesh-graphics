/**
 * Themes — named tokens templates' colours and fonts follow (ADR 0021).
 *
 * The server stores and lists themes; it never interprets one. The canonical
 * bytes are hashed BY RUST (the browser's wasm, the same encoder behind
 * `template_rev`) and land in the CAS; this route verifies the claimed hash
 * against the bytes — the same one cross-check template publish makes — and
 * keeps a tokens-JSON copy purely for listing. Token RESOLUTION happens in the
 * compiler at publish, which is what keeps a revision naming its pixels.
 */

import express from 'express'
import * as cas from '../cas.js'
import { handle } from '../db.js'
import { newId } from '../ids.js'
import { requirePermission } from '../auth.js'

const mayDesign = requirePermission('graphics.templates.edit')

export const router = express.Router()

const fail = (res, status, error, label) => res.status(status).json({ error, label })

router.get('/api/themes', (_req, res) => {
  const rows = handle()
    .prepare('SELECT * FROM themes ORDER BY name')
    .all()
    .map((r) => ({ ...r, tokens: JSON.parse(r.tokens) }))
  res.json({ themes: rows })
})

/**
 * Create or update a theme. Body:
 * `{ id?, name, tokens, hash, bytes: <base64 canonical CBOR> }`
 *
 * `hash` and `bytes` come from the wasm encoder. Verified here the way a
 * template publish is: bytes that do not hash to the claim are two halves
 * that do not belong together.
 */
router.post('/api/themes', mayDesign, async (req, res) => {
  const db = handle()
  const b = req.body ?? {}
  if (typeof b.name !== 'string' || !b.name.trim()) {
    return fail(res, 422, 'validation_failed', 'A theme needs a name.')
  }
  if (!b.tokens || typeof b.tokens !== 'object') {
    return fail(res, 422, 'validation_failed', 'A theme is its tokens; send them.')
  }
  if (typeof b.hash !== 'string' || typeof b.bytes !== 'string') {
    return fail(res, 422, 'validation_failed', 'Send the canonical bytes and their address.')
  }
  const bytes = Buffer.from(b.bytes, 'base64')
  try {
    // `expect` makes the CAS itself refuse a claim that does not match the
    // bytes — nothing lands under a lie.
    await cas.put(bytes, b.hash)
  } catch (e) {
    if (e?.code === 'hash_mismatch') {
      return fail(res, 422, 'validation_failed', `${e.message} — these halves do not belong together.`)
    }
    throw e
  }

  const now = Date.now()
  const id = b.id ?? newId()
  const cur = db.prepare('SELECT id FROM themes WHERE id = ?').get(id)
  if (cur) {
    db.prepare('UPDATE themes SET name = ?, head_hash = ?, tokens = ?, updated_at = ? WHERE id = ?').run(
      b.name.trim(),
      b.hash,
      JSON.stringify(b.tokens),
      now,
      id,
    )
  } else {
    db.prepare(
      `INSERT INTO themes (id, name, head_hash, tokens, created_at, updated_at)
       VALUES (?, ?, ?, ?, ?, ?)`,
    ).run(id, b.name.trim(), b.hash, JSON.stringify(b.tokens), now, now)
  }
  res.status(cur ? 200 : 201).json({ id, name: b.name.trim(), head_hash: b.hash })
})
