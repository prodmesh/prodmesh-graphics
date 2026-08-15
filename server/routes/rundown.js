/**
 * Playlist and Rundown routes. Spec: `docs/specs/http-api.md` §3.
 *
 * Reordering is a PUT of the whole list rather than a PATCH of one position.
 * Two operators dragging at once with per-position edits produce an order
 * neither of them chose; replacing the list wholesale means the second write
 * wins cleanly and visibly, which is the honest behaviour until there is a
 * reason to build real collaborative editing.
 */

import express from 'express'
import * as rundown from '../rundown.js'
import { handle } from '../db.js'
import { requirePermission } from '../auth.js'

const mayEdit = requirePermission('graphics.rundown.edit')

export const router = express.Router()

const fail = (res, status, error, label) => res.status(status).json({ error, label })

const idsOf = (body, key) =>
  Array.isArray(body?.[key]) && body[key].every((v) => typeof v === 'string') ? body[key] : null

// ---------------------------------------------------------------------------
// Playlists
// ---------------------------------------------------------------------------

router.get('/api/playlists', (_req, res) => res.json({ playlists: rundown.listPlaylists() }))

router.get('/api/playlists/:id', (req, res) => {
  const p = rundown.getPlaylist(req.params.id)
  if (!p) return fail(res, 404, 'not_found', 'There is no playlist with that id.')
  res.json(p)
})

router.post('/api/playlists', mayEdit, (req, res) => {
  const name = String(req.body?.name ?? '').trim()
  if (!name) return fail(res, 422, 'validation_failed', 'A playlist needs a name.')
  res.status(201).json(rundown.createPlaylist(name))
})

router.put('/api/playlists/:id/items', mayEdit, (req, res) => {
  if (!rundown.getPlaylist(req.params.id)) {
    return fail(res, 404, 'not_found', 'There is no playlist with that id.')
  }
  const ids = idsOf(req.body, 'items')
  if (!ids) return fail(res, 422, 'validation_failed', 'Send the items in the order you want them.')

  // Every id must be a real item. A playlist referring to something that does
  // not exist is a rundown with a hole in it, discovered on Sunday.
  const db = handle()
  const missing = ids.filter((id) => !db.prepare('SELECT 1 FROM items WHERE id = ?').get(id))
  if (missing.length) {
    return fail(res, 422, 'validation_failed', `${missing.length} of those items are not on this server.`)
  }
  if (new Set(ids).size !== ids.length) {
    // The same item twice in one playlist makes "take next" ambiguous about
    // where the cursor should land.
    return fail(res, 422, 'validation_failed', 'An item can only appear once in a playlist.')
  }
  res.json(rundown.setPlaylistItems(req.params.id, ids))
})

router.delete('/api/playlists/:id', mayEdit, (req, res) => {
  const info = handle().prepare('DELETE FROM playlists WHERE id = ?').run(req.params.id)
  if (!info.changes) return fail(res, 404, 'not_found', 'There is no playlist with that id.')
  res.status(204).end()
})

// ---------------------------------------------------------------------------
// Rundowns
// ---------------------------------------------------------------------------

router.get('/api/rundowns', (_req, res) => res.json({ rundowns: rundown.listRundowns() }))

router.get('/api/rundowns/:id', (req, res) => {
  const r = rundown.getRundown(req.params.id)
  if (!r) return fail(res, 404, 'not_found', 'There is no rundown with that id.')
  res.json(r)
})

router.post('/api/rundowns', mayEdit, (req, res) => {
  const name = String(req.body?.name ?? '').trim()
  if (!name) return fail(res, 422, 'validation_failed', 'A rundown needs a name.')
  res.status(201).json(rundown.createRundown(name))
})

router.put('/api/rundowns/:id/playlists', mayEdit, (req, res) => {
  if (!rundown.getRundown(req.params.id)) {
    return fail(res, 404, 'not_found', 'There is no rundown with that id.')
  }
  const ids = idsOf(req.body, 'playlists')
  if (!ids) return fail(res, 422, 'validation_failed', 'Send the playlists in the order you want them.')
  const db = handle()
  const missing = ids.filter((id) => !db.prepare('SELECT 1 FROM playlists WHERE id = ?').get(id))
  if (missing.length) {
    return fail(res, 422, 'validation_failed', `${missing.length} of those playlists are not on this server.`)
  }
  res.json(rundown.setRundownPlaylists(req.params.id, ids))
})

// ---------------------------------------------------------------------------
// The cursor — navigation, never a cue
// ---------------------------------------------------------------------------

router.get('/api/rundowns/:id/cursor', (req, res) => {
  if (!rundown.getRundown(req.params.id)) {
    return fail(res, 404, 'not_found', 'There is no rundown with that id.')
  }
  res.json(rundown.getCursor(req.params.id))
})

/**
 * Move or set the cursor.
 *
 * `{ item }` to jump, `{ delta: 1 }` to advance. **Neither takes anything.**
 * Show composes cursor movement and a cue into one button because that is what
 * an operator wants, but keeping them separate here is what lets the UI show
 * "the rundown is here, the output is showing something else" instead of
 * pretending they cannot differ.
 */
router.put('/api/rundowns/:id/cursor', requirePermission('graphics.operate'), (req, res) => {
  if (!rundown.getRundown(req.params.id)) {
    return fail(res, 404, 'not_found', 'There is no rundown with that id.')
  }
  if (typeof req.body?.delta === 'number') {
    return res.json(rundown.moveCursor(req.params.id, req.body.delta))
  }
  if (typeof req.body?.item === 'string') {
    const inOrder = rundown.order(req.params.id).some((r) => r.id === req.body.item)
    if (!inOrder) {
      return fail(res, 422, 'validation_failed', 'That item is not in this rundown.')
    }
    return res.json(rundown.setCursor(req.params.id, req.body.item))
  }
  fail(res, 422, 'validation_failed', 'Send an item to jump to, or a delta to move by.')
})
