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

/**
 * Replace a playlist's entries — its arrangement.
 *
 * Separate from `reorder` because an entry is not a membership: it carries a
 * section, and the same Item may appear more than once (ADR 0026). That is
 * what makes a chorus that comes round three times one piece of content in
 * three places.
 *
 * **An entry keeps its id when the caller sends one back.** Regenerating ids
 * on every reorder would move the operator's cursor every time somebody
 * dragged a row in Edit, which is the kind of thing that is only ever noticed
 * mid-service.
 */
function reorderEntries(db, playlistId, entries) {
  db.transaction(() => {
    const keep = new Set(
      db
        .prepare('SELECT entry_id FROM playlist_entries WHERE playlist_id = ?')
        .all(playlistId)
        .map((r) => r.entry_id),
    )
    db.prepare('DELETE FROM playlist_entries WHERE playlist_id = ?').run(playlistId)
    const insert = db.prepare(
      `INSERT INTO playlist_entries (entry_id, playlist_id, item_id, position, section)
       VALUES (?, ?, ?, ?, ?)`,
    )
    entries.forEach((e, i) => {
      const id = e.entry && keep.has(e.entry) ? e.entry : newId()
      insert.run(id, playlistId, e.item, i, e.section ?? null)
    })
  })()
}

// ---------------------------------------------------------------------------
// Playlists
// ---------------------------------------------------------------------------

export function listPlaylists() {
  const db = handle()
  return db
    .prepare(
      `SELECT p.*,
              (SELECT COUNT(*) FROM playlist_entries pe WHERE pe.playlist_id = p.id) AS items,
              (SELECT GROUP_CONCAT(tag, char(31)) FROM playlist_tags t WHERE t.playlist_id = p.id) AS tags
         FROM playlists p ORDER BY p.updated_at DESC`,
    )
    .all()
    // Unit-separator joined and split back here: SQLite has no array type, and
    // a comma would make a tag containing one into two.
    .map((p) => ({ ...p, tags: p.tags ? p.tags.split('\u001f').sort() : [] }))
}

/** Every tag anybody has typed, with how many playlists carry it. */
export function listTags() {
  return handle()
    .prepare('SELECT tag, COUNT(*) AS playlists FROM playlist_tags GROUP BY tag ORDER BY tag')
    .all()
}

export function setPlaylistTags(id, tags) {
  const db = handle()
  const clean = [...new Set(tags.map((t) => String(t).trim()).filter(Boolean))]
  db.transaction(() => {
    db.prepare('DELETE FROM playlist_tags WHERE playlist_id = ?').run(id)
    const ins = db.prepare('INSERT INTO playlist_tags (playlist_id, tag) VALUES (?, ?)')
    for (const t of clean) ins.run(id, t)
    db.prepare('UPDATE playlists SET updated_at = ? WHERE id = ?').run(Date.now(), id)
  })()
  return getPlaylist(id)
}

export function getPlaylist(id) {
  const db = handle()
  const p = db.prepare('SELECT * FROM playlists WHERE id = ?').get(id)
  if (!p) return null
  const items = db
    .prepare(
      `SELECT i.*, pe.entry_id, pe.position, pe.section FROM playlist_entries pe
         JOIN items i ON i.id = pe.item_id
        WHERE pe.playlist_id = ? ORDER BY pe.position`,
    )
    .all(id)
  const tags = db
    .prepare('SELECT tag FROM playlist_tags WHERE playlist_id = ? ORDER BY tag')
    .all(id)
    .map((r) => r.tag)
  return {
    ...p,
    tags,
    items: items.map((i) => ({
      ...i,
      data: JSON.parse(i.data),
      destination: JSON.parse(i.destination),
    })),
  }
}

/**
 * Every Item this playlist owns, whether or not the arrangement uses it.
 *
 * A chorus dropped from tonight's arrangement is still the song's chorus, and
 * losing it because nothing currently points at it would make removing a row
 * a destructive act nobody expects.
 */
export function playlistLibrary(id) {
  return handle()
    .prepare('SELECT * FROM items WHERE playlist_id = ? ORDER BY created_at')
    .all(id)
    .map((i) => ({ ...i, data: JSON.parse(i.data), destination: JSON.parse(i.destination) }))
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

/** Replace a playlist's arrangement — which Items, in what order, repeats and all. */
export function setPlaylistItems(id, entries) {
  const db = handle()
  // A bare id list is still accepted: most playlists have no sections, and
  // requiring every caller to wrap ids in objects would be ceremony for the
  // common case.
  const normalised = entries.map((e) => (typeof e === 'string' ? { item: e } : e))
  reorderEntries(db, id, normalised)
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

export function renamePlaylist(id, name) {
  const info = handle()
    .prepare('UPDATE playlists SET name = ?, updated_at = ? WHERE id = ?')
    .run(name, Date.now(), id)
  return info.changes ? getPlaylist(id) : null
}

export function renameRundown(id, name) {
  const info = handle()
    .prepare('UPDATE rundowns SET name = ?, updated_at = ? WHERE id = ?')
    .run(name, Date.now(), id)
  return info.changes ? getRundown(id) : null
}

export function deleteRundown(id) {
  // The playlists survive: a rundown is an ORDER over them, and deleting
  // Sunday's running order must not delete Sunday's content.
  return handle().prepare('DELETE FROM rundowns WHERE id = ?').run(id).changes > 0
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
      `SELECT pe.entry_id, i.id, i.label, pe.section, pe.playlist_id, p.name AS playlist_name
         FROM rundown_playlists rp
         JOIN playlists p ON p.id = rp.playlist_id
         JOIN playlist_entries pe ON pe.playlist_id = rp.playlist_id
         JOIN items i ON i.id = pe.item_id
        WHERE rp.rundown_id = ?
        ORDER BY rp.position, pe.position`,
    )
    .all(rundownId)
}

/**
 * Where the operator is, as an ENTRY rather than an Item (ADR 0026).
 *
 * With repeats, "the cursor is on the chorus" is not an answer: a chorus that
 * comes round three times is at three places in the running order, and finding
 * it by item id finds the first one — so advancing past the second would jump
 * backwards to the first. The entry is the position; the item is what is there.
 *
 * `item` is still reported, because every consumer takes by item and none of
 * them should have to learn a second identifier to do it.
 */
export function getCursor(rundownId) {
  const row = handle()
    .prepare('SELECT entry_id, item_id FROM rundown_cursor WHERE rundown_id = ?')
    .get(rundownId)
  const running = order(rundownId)
  // No cursor row at all — nobody has navigated yet — starts at the top.
  // A cursor that HAS been set and no longer resolves reports -1 rather than
  // sliding to a neighbour: moving an operator's place without telling them is
  // worse than saying honestly that the thing they were on is gone.
  let at = row ? -1 : running.length ? 0 : -1
  if (row?.entry_id) at = running.findIndex((r) => r.entry_id === row.entry_id)
  // Falling back to the item covers a cursor written before entries existed,
  // and an entry rewritten by a reorder that did not send its id back.
  if (at < 0 && row?.item_id) at = running.findIndex((r) => r.id === row.item_id)
  const here = at >= 0 ? running[at] : null
  return {
    rundown: rundownId,
    entry: here?.entry_id ?? null,
    item: here?.id ?? null,
    index: at,
    total: running.length,
    playlist: here?.playlist_id ?? null,
    label: here?.label ?? null,
    // `next` and `previous` are ITEM ids: every consumer of them fires a cue,
    // and a cue names an item. The entry variants beside them are where the
    // cursor would LAND, which is a different question the moment an item is
    // in the running order twice.
    next: at >= 0 && at + 1 < running.length ? running[at + 1].id : null,
    previous: at > 0 ? running[at - 1].id : null,
    next_entry: at >= 0 && at + 1 < running.length ? running[at + 1].entry_id : null,
    previous_entry: at > 0 ? running[at - 1].entry_id : null,
  }
}

/**
 * Put the cursor somewhere. Accepts an entry id or an item id.
 *
 * An item id still works because Show takes by item and a GPI surface names
 * one; it lands on the FIRST entry holding that item, which is the only
 * defensible reading of an ambiguous instruction.
 */
export function setCursor(rundownId, target) {
  const running = order(rundownId)
  const found =
    running.find((r) => r.entry_id === target) ?? running.find((r) => r.id === target) ?? null
  handle()
    .prepare(
      `INSERT INTO rundown_cursor (rundown_id, item_id, entry_id, updated_at) VALUES (?, ?, ?, ?)
       ON CONFLICT(rundown_id) DO UPDATE SET
         item_id = excluded.item_id, entry_id = excluded.entry_id, updated_at = excluded.updated_at`,
    )
    .run(rundownId, found?.id ?? target, found?.entry_id ?? null, Date.now())
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
  return setCursor(rundownId, running[next].entry_id)
}

export function registerTopics() {
  registerTopic('rundown:*:cursor', {
    valid: ([id]) => Boolean(handle().prepare('SELECT 1 FROM rundowns WHERE id = ?').get(id)),
    start: () => null,
    stop: () => {},
    snapshot: ([id]) => getCursor(id),
  })
}
