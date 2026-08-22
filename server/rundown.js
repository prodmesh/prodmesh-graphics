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
 * Replace a playlist's entries — Main, the order that owns the slides.
 *
 * Dense integers, rewritten every time. A playlist is tens of entries, so the
 * cost is nothing and there is exactly one order with no gaps to reason about —
 * as opposed to fractional ranks, which drift toward collision and need a
 * compaction pass nobody remembers to write.
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

export function getPlaylist(id, arrangementId = null) {
  const db = handle()
  const p = db.prepare('SELECT * FROM playlists WHERE id = ?').get(id)
  if (!p) return null
  const byId = new Map(
    db.prepare('SELECT * FROM items WHERE playlist_id = ?').all(id).map((i) => [i.id, i]),
  )
  const items = expand(id, arrangementId)
    .map((e) => {
      const item = byId.get(e.item_id)
      return item ? { ...item, entry_id: e.entry_id, position: e.position, section: e.section } : null
    })
    .filter(Boolean)
  const tags = db
    .prepare('SELECT tag FROM playlist_tags WHERE playlist_id = ? ORDER BY tag')
    .all(id)
    .map((r) => r.tag)
  return {
    ...p,
    tags,
    arrangement: arrangementId ?? null,
    arrangements: listArrangements(id),
    groups: groupsOf(id).map((g) => ({ name: g.name, slides: g.entries.length })),
    ungrouped: ungroupedIn(id),
    items: items.map((i) => ({
      ...i,
      data: JSON.parse(i.data),
      destination: JSON.parse(i.destination),
    })),
  }
}

/**
 * The groups Main defines, in Main's order.
 *
 * A group is the **first consecutive run** of a section name. Later runs of the
 * same name are Main repeating that group, not more slides belonging to it —
 * a chorus sung twice in Main is the same four slides twice, and folding both
 * runs into one group would make every arrangement sing an eight-slide chorus.
 *
 * Entries with no section belong to no group. They cannot be arranged, and the
 * browser says so rather than dropping them quietly (ADR 0027).
 */
export function groupsOf(playlistId) {
  const rows = handle()
    .prepare(
      `SELECT pe.entry_id, pe.item_id, pe.section, pe.position, i.label
         FROM playlist_entries pe JOIN items i ON i.id = pe.item_id
        WHERE pe.playlist_id = ? ORDER BY pe.position`,
    )
    .all(playlistId)

  const groups = []
  const seen = new Set()
  let run = null
  for (const r of rows) {
    if (!r.section) {
      run = null
      continue
    }
    if (run?.name !== r.section) {
      run = seen.has(r.section) ? { name: r.section, entries: null } : { name: r.section, entries: [] }
      if (run.entries) {
        seen.add(r.section)
        groups.push(run)
      }
    }
    run.entries?.push(r)
  }
  return groups.map((g) => ({ name: g.name, entries: g.entries }))
}

/** Entries in this playlist that no group contains — unarrangeable, by name. */
export function ungroupedIn(playlistId) {
  return handle()
    .prepare(
      `SELECT i.label FROM playlist_entries pe JOIN items i ON i.id = pe.item_id
        WHERE pe.playlist_id = ? AND (pe.section IS NULL OR pe.section = '')
        ORDER BY pe.position`,
    )
    .all(playlistId)
    .map((r) => r.label)
}

export function listArrangements(playlistId) {
  const db = handle()
  return db
    .prepare('SELECT * FROM arrangements WHERE playlist_id = ? ORDER BY created_at')
    .all(playlistId)
    .map((a) => ({
      ...a,
      groups: db
        .prepare('SELECT section FROM arrangement_groups WHERE arrangement_id = ? ORDER BY position')
        .all(a.id)
        .map((r) => r.section),
    }))
}

export function createArrangement(playlistId, name, id = newId()) {
  handle()
    .prepare('INSERT INTO arrangements (id, playlist_id, name, created_at) VALUES (?, ?, ?, ?)')
    .run(id, playlistId, name, Date.now())
  // A new arrangement starts as Main's groups in Main's order. An empty one is
  // a song that plays nothing, and nobody means that.
  const groups = groupsOf(playlistId).map((g) => g.name)
  setArrangementGroups(id, groups)
  return listArrangements(playlistId).find((a) => a.id === id)
}

export function setArrangementGroups(id, groups) {
  const db = handle()
  db.transaction(() => {
    db.prepare('DELETE FROM arrangement_groups WHERE arrangement_id = ?').run(id)
    const ins = db.prepare(
      'INSERT INTO arrangement_groups (arrangement_id, position, section) VALUES (?, ?, ?)',
    )
    groups.forEach((g, i) => ins.run(id, i, g))
  })()
}

/**
 * A playlist's running order under one arrangement.
 *
 * `null` is Main — the entries themselves. A named arrangement expands its
 * group list into Main's slides, minting a stable id per OCCURRENCE: the same
 * group twice is the same slides twice, and the cursor has to be able to tell
 * the two apart (ADR 0026's whole reason for entries).
 */
export function expand(playlistId, arrangementId) {
  const db = handle()
  if (!arrangementId) {
    return db
      .prepare(
        `SELECT pe.entry_id, pe.item_id, pe.section, pe.position
           FROM playlist_entries pe WHERE pe.playlist_id = ? ORDER BY pe.position`,
      )
      .all(playlistId)
  }
  const names = db
    .prepare('SELECT section FROM arrangement_groups WHERE arrangement_id = ? ORDER BY position')
    .all(arrangementId)
    .map((r) => r.section)
  const groups = new Map(groupsOf(playlistId).map((g) => [g.name, g.entries]))
  const out = []
  names.forEach((name, occurrence) => {
    for (const e of groups.get(name) ?? []) {
      out.push({
        // Deterministic, so the cursor survives a reload; distinct per
        // occurrence, so it survives a repeat.
        entry_id: `${arrangementId}:${occurrence}:${e.entry_id}`,
        item_id: e.item_id,
        section: e.section,
        position: out.length,
      })
    }
  })
  return out
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
  const rows = db
    .prepare('SELECT playlist_id, arrangement_id FROM rundown_playlists WHERE rundown_id = ? ORDER BY position')
    .all(id)
  return {
    ...r,
    // Each inclusion carries its own arrangement: 09:00 short, 11:00 full, one
    // song (ADR 0027). NULL is Main and is what most of them mean.
    playlists: rows.map((row) => getPlaylist(row.playlist_id, row.arrangement_id)).filter(Boolean),
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

/**
 * Replace a rundown's playlists, each optionally pinned to an arrangement.
 *
 * Accepts bare ids or `{ playlist, arrangement }`. An arrangement that does not
 * belong to the playlist it is pinned against is dropped rather than stored:
 * a rundown pointing at another song's arrangement would expand to nothing on
 * the morning it mattered.
 */
export function setRundownPlaylists(id, entries) {
  const db = handle()
  const rows = entries.map((e) => (typeof e === 'string' ? { playlist: e, arrangement: null } : e))
  db.transaction(() => {
    db.prepare('DELETE FROM rundown_playlists WHERE rundown_id = ?').run(id)
    const ins = db.prepare(
      'INSERT INTO rundown_playlists (rundown_id, playlist_id, position, arrangement_id) VALUES (?, ?, ?, ?)',
    )
    rows.forEach((r, i) => {
      const ok =
        r.arrangement &&
        db
          .prepare('SELECT 1 FROM arrangements WHERE id = ? AND playlist_id = ?')
          .get(r.arrangement, r.playlist)
      ins.run(id, r.playlist, i, ok ? r.arrangement : null)
    })
  })()
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
      'SELECT playlist_id, arrangement_id FROM rundown_playlists WHERE rundown_id = ? ORDER BY position',
    )
    .all(rundownId)
    .flatMap((rp) => {
      const p = handle().prepare('SELECT name FROM playlists WHERE id = ?').get(rp.playlist_id)
      const labels = new Map(
        handle()
          .prepare('SELECT id, label FROM items WHERE playlist_id = ?')
          .all(rp.playlist_id)
          .map((i) => [i.id, i.label]),
      )
      return expand(rp.playlist_id, rp.arrangement_id).map((e) => ({
        entry_id: e.entry_id,
        id: e.item_id,
        label: labels.get(e.item_id) ?? e.item_id,
        section: e.section,
        playlist_id: rp.playlist_id,
        playlist_name: p?.name ?? null,
      }))
    })
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
