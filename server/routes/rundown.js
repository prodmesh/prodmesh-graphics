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

/** Every tag anybody has typed, with how many playlists carry it. */
router.get('/api/tags', (_req, res) => res.json({ tags: rundown.listTags() }))

/**
 * Replace a playlist's tags.
 *
 * Free strings and no tag table: a vocabulary nobody can add to without an
 * admin screen is a vocabulary nobody uses. The set on offer is whatever has
 * been typed (`GET /api/tags`).
 */
router.put('/api/playlists/:id/tags', mayEdit, (req, res) => {
  if (!rundown.getPlaylist(req.params.id)) {
    return fail(res, 404, 'not_found', 'There is no playlist with that id.')
  }
  const tags = Array.isArray(req.body?.tags) ? req.body.tags : null
  if (!tags) return fail(res, 422, 'validation_failed', 'Send the tags as a list.')
  res.json(rundown.setPlaylistTags(req.params.id, tags))
})

router.get('/api/playlists/:id', (req, res) => {
  const p = rundown.getPlaylist(req.params.id)
  if (!p) return fail(res, 404, 'not_found', 'There is no playlist with that id.')
  res.json(p)
})

/** The one place a mode string from outside is judged. */
const badMode = (mode) =>
  rundown.PLAYLIST_MODES.includes(mode)
    ? null
    : `There is no playout mode called “${mode}”. Use ${rundown.PLAYLIST_MODES.join(' or ')}.`

router.post('/api/playlists', mayEdit, (req, res) => {
  const name = String(req.body?.name ?? '').trim()
  if (!name) return fail(res, 422, 'validation_failed', 'A playlist needs a name.')
  // Defaults to `cue`, the mode with an arm step. A playlist that silently came
  // up in `follow` would put a graphic on air the first time someone clicked a
  // row to see what was in it.
  const mode = req.body?.mode ?? 'cue'
  const problem = badMode(mode)
  if (problem) return fail(res, 422, 'validation_failed', problem)
  res.status(201).json(rundown.createPlaylist(name, undefined, mode))
})

/**
 * `PUT /api/playlists/:id/mode` — change what navigating means here.
 *
 * Separate from a rename on purpose: this changes what the arrow keys DO for
 * whoever is running the show, which is worth an explicit act rather than a
 * field that rides along with an edit nobody thought was dangerous.
 */
router.patch('/api/playlists/:id', mayEdit, (req, res) => {
  const name = String(req.body?.name ?? '').trim()
  if (!name) return fail(res, 422, 'validation_failed', 'A playlist needs a name.')
  const p = rundown.renamePlaylist(req.params.id, name)
  if (!p) return fail(res, 404, 'not_found', 'There is no playlist with that id.')
  res.json(p)
})

router.put('/api/playlists/:id/mode', mayEdit, (req, res) => {
  const mode = String(req.body?.mode ?? '')
  const problem = badMode(mode)
  if (problem) return fail(res, 422, 'validation_failed', problem)
  const p = rundown.setPlaylistMode(req.params.id, mode)
  if (!p) return fail(res, 404, 'not_found', 'There is no playlist with that id.')
  res.json(p)
})

router.put('/api/playlists/:id/items', mayEdit, (req, res) => {
  if (!rundown.getPlaylist(req.params.id)) {
    return fail(res, 404, 'not_found', 'There is no playlist with that id.')
  }
  // Either a bare id list or `{item, section}` entries. Sections are optional
  // because a sermon deck has none, and a song has one per slide.
  const raw = Array.isArray(req.body?.items) ? req.body.items : null
  if (!raw) return fail(res, 422, 'validation_failed', 'Send the items in the order you want them.')
  const entries = raw.map((e) =>
    typeof e === 'string'
      ? { item: e }
      : { entry: e?.entry ?? null, item: e?.item, section: e?.section ?? null },
  )
  const ids = entries.map((e) => e.item)
  if (ids.some((id) => typeof id !== 'string' || !id)) {
    return fail(res, 422, 'validation_failed', 'Every entry needs an item id.')
  }

  // Every id must be a real item. A playlist referring to something that does
  // not exist is a rundown with a hole in it, discovered on Sunday.
  const db = handle()
  const missing = ids.filter((id) => !db.prepare('SELECT 1 FROM items WHERE id = ?').get(id))
  if (missing.length) {
    return fail(res, 422, 'validation_failed', `${missing.length} of those items are not on this server.`)
  }
  // The same Item twice is now the POINT (ADR 0026): a chorus that comes round
  // three times is one piece of content in three places, so a typo is fixed
  // once. It used to be refused because the cursor was addressed by item and
  // "take next" had no answer; the cursor addresses an ENTRY now.
  //
  // Every Item must belong to THIS playlist. Arranging somebody else's content
  // into your song is how one edit silently changes two services.
  const foreign = ids.filter((id) => {
    const owner = db.prepare('SELECT playlist_id FROM items WHERE id = ?').get(id)?.playlist_id
    return owner && owner !== req.params.id
  })
  if (foreign.length) {
    return fail(
      res,
      422,
      'validation_failed',
      'An item belongs to the playlist it was written for. Copy it into this one instead.',
    )
  }
  res.json(rundown.setPlaylistItems(req.params.id, entries))
})

/**
 * Delete a playlist AND the Items it owns (ADR 0026).
 *
 * Cascading is right now that an Item belongs to one playlist: leaving its
 * items behind would leave content nothing can reach. The browser says how
 * many before it asks.
 */
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

router.patch('/api/rundowns/:id', mayEdit, (req, res) => {
  const name = String(req.body?.name ?? '').trim()
  if (!name) return fail(res, 422, 'validation_failed', 'A rundown needs a name.')
  const r = rundown.renameRundown(req.params.id, name)
  if (!r) return fail(res, 404, 'not_found', 'There is no rundown with that id.')
  res.json(r)
})

/**
 * Delete a rundown. Its playlists are untouched — a rundown is an ORDER over
 * them, and deleting Sunday's running order must not delete Sunday's content.
 */
router.delete('/api/rundowns/:id', mayEdit, (req, res) => {
  if (!rundown.deleteRundown(req.params.id)) {
    return fail(res, 404, 'not_found', 'There is no rundown with that id.')
  }
  res.status(204).end()
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
  // Jump to a named section — the rundown's *"worship leader suddenly decides
  // to sing the Chorus again. Operator clicks the first slide in the Chorus"*.
  //
  // Resolved to that section's FIRST entry rather than being a cursor position
  // of its own: the cursor stays one thing, an item, and a section is a way of
  // naming one. A section that appears twice resolves to its first occurrence,
  // which is what "go back to the chorus" means.
  if (typeof req.body?.section === 'string') {
    const first = rundown
      .order(req.params.id)
      .find((r) => r.section === req.body.section)
    if (!first) {
      return fail(res, 422, 'validation_failed', `This rundown has no section called “${req.body.section}”.`)
    }
    return res.json(rundown.setCursor(req.params.id, first.entry_id))
  }
  // An ENTRY is the unambiguous jump: with repeats, an item id names a place
  // only when it appears once. Both are accepted — Show clicks an entry, a
  // hardware surface knows an item — and an item lands on its first entry.
  if (typeof req.body?.entry === 'string') {
    const inOrder = rundown.order(req.params.id).some((r) => r.entry_id === req.body.entry)
    if (!inOrder) {
      return fail(res, 422, 'validation_failed', 'That entry is not in this rundown.')
    }
    return res.json(rundown.setCursor(req.params.id, req.body.entry))
  }
  if (typeof req.body?.item === 'string') {
    const inOrder = rundown.order(req.params.id).some((r) => r.id === req.body.item)
    if (!inOrder) {
      return fail(res, 422, 'validation_failed', 'That item is not in this rundown.')
    }
    return res.json(rundown.setCursor(req.params.id, req.body.item))
  }
  fail(res, 422, 'validation_failed', 'Send an item, an entry or a section to jump to, or a delta to move by.')
})
