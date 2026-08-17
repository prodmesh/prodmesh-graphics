/**
 * Playlists, the rundown, and the cursor.
 *
 * # What each word means, and it is the vocabulary in the UI too
 *
 * A **Playlist** is an ordered list of Items — one segment of a service.
 * A **Rundown** is the ordered list of Playlists that makes up the whole
 * service. The **cursor** is the operator's place in it.
 *
 * # The cursor is intent, not fact
 *
 * It records what the operator means to do next. **What is actually on air is
 * the node's answer**, and it lives nowhere in this module. The two are allowed
 * to disagree — an operator takes a graphic from the rack without touching the
 * rundown, a GPI fires at the node, a cue is rejected — and when they do, the
 * UI shows the divergence rather than reconciling it silently
 * (`docs/THEORY-OF-OPERATIONS.md`).
 *
 * Advancing the cursor therefore does NOT take anything, and taking something
 * does not have to move the cursor. Show composes the two into one button
 * because that is what an operator wants; keeping them separate here is what
 * makes the divergence representable at all.
 */

import { handle } from './db.js'
import { publish, registerTopic } from './streamHub.js'
import { newId } from './ids.js'

/**
 * Renumber a list densely, in one transaction.
 *
 * Dense integers, rewritten on every reorder. A rundown is tens of entries, so
 * the cost is nothing and there is exactly one order with no gaps to reason
 * about — as opposed to fractional ranks, which drift toward collision and need
 * a compaction pass nobody remembers to write.
 */
function reorder(db, table, keyColumn, keyValue, memberColumn, members) {
  db.transaction(() => {
    db.prepare(`DELETE FROM ${table} WHERE ${keyColumn} = ?`).run(keyValue)
    const insert = db.prepare(
      `INSERT INTO ${table} (${keyColumn}, ${memberColumn}, position) VALUES (?, ?, ?)`,
    )
    members.forEach((m, i) => insert.run(keyValue, m, i))
  })()
}

// ---------------------------------------------------------------------------
// Playlists
// ---------------------------------------------------------------------------

export function listPlaylists() {
  const db = handle()
  return db
    .prepare(
      `SELECT p.*, (SELECT COUNT(*) FROM playlist_items pi WHERE pi.playlist_id = p.id) AS items
         FROM playlists p ORDER BY p.updated_at DESC`,
    )
    .all()
}

export function getPlaylist(id) {
  const db = handle()
  const p = db.prepare('SELECT * FROM playlists WHERE id = ?').get(id)
  if (!p) return null
  const items = db
    .prepare(
      `SELECT i.*, pi.position FROM playlist_items pi
         JOIN items i ON i.id = pi.item_id
        WHERE pi.playlist_id = ? ORDER BY pi.position`,
    )
    .all(id)
  return {
    ...p,
    items: items.map((i) => ({
      ...i,
      data: JSON.parse(i.data),
      destination: JSON.parse(i.destination),
    })),
  }
}

/**
 * The playout modes this server knows (ADR 0017).
 *
 * `cue`    — the cursor is what I intend to arm. Taking is a separate action.
 * `follow` — the cursor is where I am in the content. Navigating IS taking.
 *
 * A list rather than a boolean because a third is anticipated (timed
 * auto-advance, a countdown roll) and the seam only works if adding one is an
 * addition here rather than a migration everywhere.
 */
export const PLAYLIST_MODES = ['cue', 'follow']

export function createPlaylist(name, id = newId(), mode = 'cue') {
  const now = Date.now()
  handle()
    .prepare('INSERT INTO playlists (id, name, mode, created_at, updated_at) VALUES (?, ?, ?, ?, ?)')
    .run(id, name, mode, now, now)
  return { id, name, mode, created_at: now, updated_at: now, items: [] }
}

/**
 * Change what navigating means in this playlist.
 *
 * Its own endpoint rather than a field on a general update, because it changes
 * what the operator's arrow keys DO. That is worth an explicit act and an
 * explicit permission check, not something that rides along with a rename.
 */
export function setPlaylistMode(id, mode) {
  const info = handle()
    .prepare('UPDATE playlists SET mode = ?, updated_at = ? WHERE id = ?')
    .run(mode, Date.now(), id)
  return info.changes ? getPlaylist(id) : null
}

/** Replace a playlist's contents and order in one go. */
export function setPlaylistItems(id, itemIds) {
  const db = handle()
  reorder(db, 'playlist_items', 'playlist_id', id, 'item_id', itemIds)
  db.prepare('UPDATE playlists SET updated_at = ? WHERE id = ?').run(Date.now(), id)
  return getPlaylist(id)
}

// ---------------------------------------------------------------------------
// Rundowns
// ---------------------------------------------------------------------------

export function listRundowns() {
  return handle().prepare('SELECT * FROM rundowns ORDER BY updated_at DESC').all()
}

export function getRundown(id) {
  const db = handle()
  const r = db.prepare('SELECT * FROM rundowns WHERE id = ?').get(id)
  if (!r) return null
  const playlistIds = db
    .prepare('SELECT playlist_id FROM rundown_playlists WHERE rundown_id = ? ORDER BY position')
    .all(id)
    .map((row) => row.playlist_id)
  return {
    ...r,
    playlists: playlistIds.map(getPlaylist).filter(Boolean),
    cursor: getCursor(id),
  }
}

export function createRundown(name, id = newId()) {
  const now = Date.now()
  handle()
    .prepare('INSERT INTO rundowns (id, name, created_at, updated_at) VALUES (?, ?, ?, ?)')
    .run(id, name, now, now)
  return { id, name, created_at: now, updated_at: now, playlists: [], cursor: null }
}

export function setRundownPlaylists(id, playlistIds) {
  const db = handle()
  reorder(db, 'rundown_playlists', 'rundown_id', id, 'playlist_id', playlistIds)
  db.prepare('UPDATE rundowns SET updated_at = ? WHERE id = ?').run(Date.now(), id)
  return getRundown(id)
}

// ---------------------------------------------------------------------------
// The cursor
// ---------------------------------------------------------------------------

/** The rundown flattened to its running order — every item, in sequence. */
export function order(rundownId) {
  return handle()
    .prepare(
      `SELECT i.id, i.label, pi.playlist_id, p.name AS playlist_name
         FROM rundown_playlists rp
         JOIN playlists p ON p.id = rp.playlist_id
         JOIN playlist_items pi ON pi.playlist_id = rp.playlist_id
         JOIN items i ON i.id = pi.item_id
        WHERE rp.rundown_id = ?
        ORDER BY rp.position, pi.position`,
    )
    .all(rundownId)
}

export function getCursor(rundownId) {
  const row = handle()
    .prepare('SELECT item_id FROM rundown_cursor WHERE rundown_id = ?')
    .get(rundownId)
  const running = order(rundownId)
  const itemId = row?.item_id ?? running[0]?.id ?? null
  const at = running.findIndex((r) => r.id === itemId)
  return {
    rundown: rundownId,
    item: itemId,
    index: at,
    total: running.length,
    playlist: at >= 0 ? running[at].playlist_id : null,
    label: at >= 0 ? running[at].label : null,
    next: at >= 0 && at + 1 < running.length ? running[at + 1].id : null,
    previous: at > 0 ? running[at - 1].id : null,
  }
}

export function setCursor(rundownId, itemId) {
  handle()
    .prepare(
      `INSERT INTO rundown_cursor (rundown_id, item_id, updated_at) VALUES (?, ?, ?)
       ON CONFLICT(rundown_id) DO UPDATE SET item_id = excluded.item_id, updated_at = excluded.updated_at`,
    )
    .run(rundownId, itemId, Date.now())
  const cursor = getCursor(rundownId)
  publish(`rundown:${rundownId}:cursor`, cursor)
  return cursor
}

/**
 * Move the cursor by `delta` positions in the running order.
 *
 * **This does not take anything.** Advancing is navigation; taking is a cue.
 * Fusing them here would make "I moved to the wrong place" indistinguishable
 * from "I put the wrong graphic on air".
 */
export function moveCursor(rundownId, delta) {
  const running = order(rundownId)
  if (running.length === 0) return getCursor(rundownId)
  const current = getCursor(rundownId)
  const at = current.index < 0 ? 0 : current.index
  // Clamped, not wrapped. Falling off the end of a service and landing back at
  // the welcome slide is a worse surprise than a button that does nothing.
  const next = Math.max(0, Math.min(running.length - 1, at + delta))
  return setCursor(rundownId, running[next].id)
}

export function registerTopics() {
  registerTopic('rundown:*:cursor', {
    valid: ([id]) => Boolean(handle().prepare('SELECT 1 FROM rundowns WHERE id = ?').get(id)),
    start: () => null,
    stop: () => {},
    snapshot: ([id]) => getCursor(id),
  })
}
