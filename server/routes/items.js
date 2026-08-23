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
import { reofferAll } from '../nodeLink.js'
import * as screens from '../screens.js'

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
  const rows = db
    .prepare(
      `SELECT ordinal, node_id, output, layer, template_rev, mode, blank, screen_id
         FROM item_destinations WHERE item_id = ? ORDER BY ordinal`,
    )
    .all(itemId)
  // Resolved, and carrying the screen's NAME: an Item's destination list should
  // read "LED Wall" to whoever opens it, not `booth-1 / wall / Graphics`.
  return screens.resolveAll(rows).map((d) => ({ ...d, blank: !!d.blank }))
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

  return list.map((d, i) => {
    // A destination may name a SCREEN instead of the wiring (ADR 0023). The
    // triple is still written beside it: it is what resolution falls back to
    // for a row orphaned by database surgery, so a graphic goes where it used
    // to go rather than nowhere.
    const screen = d.screen_id ? screens.get(d.screen_id) : null
    return {
      ordinal: i,
      screen_id: screen ? screen.id : null,
      node_id: screen ? (screen.node_id ?? null) : (d.node_id ?? d.node ?? null),
      output: String(screen ? screen.output : (d.output ?? 'out-0')),
      layer: String(screen ? screen.layer : (d.layer ?? 'Lower Third')),
      // The screen supplies the DEFAULT treatment; the Item still pins the
      // revision, or a publish would silently change every graphic that goes
      // there. An explicit `template_rev` in the request still wins — that is
      // how the revision UI moves an Item forward.
      template_rev: d.template_rev ?? screens.defaultRevFor(d.screen_id) ?? fallbackRev,
      mode: d.mode === 'on_request' ? 'on_request' : 'auto',
      // A blank destination renders nothing HERE, deliberately — clearing that
      // feed is part of what taking this Item means.
      blank: d.blank ? 1 : 0,
    }
  })
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
  // Every field ANY destination declares. One Item, several templates, each
  // wanting what it wants (ADR 0018) — a lower third showing only the lyric and
  // a wall showing the lyric and the song title is the ordinary case, and
  // before this it could not be saved at all: each template refused the other's
  // fields as typos. Blank destinations render nothing, so they contribute
  // nothing.
  const known = new Set()
  for (const d of dests) {
    if (d.blank) continue
    for (const f of revision(db, d.template_rev)?.manifest?.fields ?? []) known.add(f.key)
  }

  for (const d of dests) {
    if (d.blank) continue
    const r = revision(db, d.template_rev)
    if (!r) {
      problems.push(`Destination ${d.output} · ${d.layer} names a template revision this server does not have.`)
      continue
    }
    // REQUIRED is still per template: a wall that needs a title and does not
    // have one is a hole in the wall, whatever the lower third thinks.
    for (const p of validateItemData(r.manifest, data, { hasAsset: cas.has, alsoKnown: known })) {
      problems.push(`${d.output} · ${d.layer}: ${p}`)
    }
  }
  return problems
}

function writeDestinations(db, itemId, dests) {
  db.prepare('DELETE FROM item_destinations WHERE item_id = ?').run(itemId)
  const ins = db.prepare(
    `INSERT INTO item_destinations (item_id, ordinal, node_id, output, layer, template_rev, mode, blank, screen_id)
     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)`,
  )
  for (const d of dests) {
    ins.run(itemId, d.ordinal, d.node_id, d.output, d.layer, d.template_rev, d.mode, d.blank, d.screen_id ?? null)
  }
}

/**
 * Items, optionally filtered.
 *
 * `?playlist=` — one playlist's Items, which is the normal case now that an
 * Item belongs to one (ADR 0026).
 * `?q=` — search by label and by the text of any field, across every playlist,
 * with the playlist named. This is what replaced "All items": a hundred songs
 * of thirty slides is thousands of rows, and a list that long is not a way to
 * find anything. A capped result set, because a search that returns everything
 * has not searched.
 */
router.get('/api/items', (req, res) => {
  const db = handle()
  const q = String(req.query.q ?? '').trim().toLowerCase()
  let rows
  if (req.query.template) {
    rows = db.prepare('SELECT * FROM items WHERE template_id = ? ORDER BY id').all(req.query.template)
  } else if (req.query.playlist) {
    rows = db.prepare('SELECT * FROM items WHERE playlist_id = ? ORDER BY created_at').all(req.query.playlist)
  } else {
    rows = db.prepare('SELECT * FROM items ORDER BY id').all()
  }
  if (q) {
    // Label first, then the values a person actually typed into the Item. The
    // field KEYS are not searched: matching "title" against every item that has
    // a title field would bury the one whose title says what was asked for.
    rows = rows.filter((r) => {
      if (r.label.toLowerCase().includes(q)) return true
      try {
        return Object.values(JSON.parse(r.data)).some((v) => String(v).toLowerCase().includes(q))
      } catch {
        return false
      }
    })
    rows = rows.slice(0, 200)
  }
  const names = new Map(db.prepare('SELECT id, name FROM playlists').all().map((p) => [p.id, p.name]))
  res.json({
    items: rows.map((r) => ({
      ...hydrate(r, db),
      playlist_name: r.playlist_id ? (names.get(r.playlist_id) ?? null) : null,
    })),
  })
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

  // An Item belongs to a playlist (ADR 0026). Refused rather than defaulted:
  // an Item created into nowhere is content that exists and cannot be reached,
  // which is the state migration 16 had to clean up once already.
  const playlistId = String(body.playlist_id ?? '').trim()
  if (!playlistId || !db.prepare('SELECT 1 FROM playlists WHERE id = ?').get(playlistId)) {
    return fail(res, 422, 'validation_failed', 'An item belongs to a playlist. Choose one to add it to.')
  }

  const id = body.id ?? newId()
  const now = Date.now()
  const row = {
    id,
    playlist_id: playlistId,
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
    `INSERT INTO items (id, playlist_id, take_id, label, template_id, template_rev, data, destination,
                        created_at, updated_at)
     VALUES (@id, @playlist_id, @take_id, @label, @template_id, @template_rev, @data, @destination,
             @created_at, @updated_at)`,
  ).run(row)
  writeDestinations(db, id, dests)
  // A new Item lands at the end of its playlist's arrangement. An Item that
  // exists but is nowhere in the order is one somebody has to go and find.
  if (body.arrange !== false) {
    const next = db
      .prepare('SELECT COALESCE(MAX(position), -1) + 1 AS n FROM playlist_entries WHERE playlist_id = ?')
      .get(playlistId).n
    db.prepare(
      `INSERT INTO playlist_entries (entry_id, playlist_id, item_id, position, section)
       VALUES (?, ?, ?, ?, NULL)`,
    ).run(newId(), playlistId, id, next)
  }
  // Content changed: nodes whose assignment this touches get the new one NOW,
  // not at their next reconnect. The digest short-circuit keeps this from
  // churning nodes the change does not reach.
  reofferAll()
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
  reofferAll()
  res.json(hydrate(db.prepare('SELECT * FROM items WHERE id = ?').get(cur.id), db))
})

/**
 * Copy an Item into another playlist, or move it there.
 *
 * A copy is a NEW Item (ADR 0026). Two songs that both say "Verse 1" are two
 * pieces of content, and a shared instance across playlists is how a fix to
 * tonight's set silently edits last week's. Sharing is right INSIDE one
 * playlist — a chorus that comes round three times — and that is repeats, not
 * this.
 *
 * A move takes the Item and its places in the arrangement with it, because
 * leaving entries behind would leave the old playlist pointing at content it no
 * longer owns.
 */
for (const verb of ['copy', 'move']) {
  router.post(`/api/items/:id/${verb}`, mayEdit, (req, res) => {
    const db = handle()
    const item = db.prepare('SELECT * FROM items WHERE id = ?').get(req.params.id)
    if (!item) return fail(res, 404, 'not_found', 'There is no item with that id.')

    const to = String(req.body?.playlist_id ?? '').trim()
    if (!to || !db.prepare('SELECT 1 FROM playlists WHERE id = ?').get(to)) {
      return fail(res, 422, 'validation_failed', 'Choose a playlist to put it in.')
    }
    if (to === item.playlist_id) {
      return fail(res, 422, 'validation_failed', 'It is already in that playlist.')
    }

    const now = Date.now()
    const append = (itemId) => {
      const next = db
        .prepare('SELECT COALESCE(MAX(position), -1) + 1 AS n FROM playlist_entries WHERE playlist_id = ?')
        .get(to).n
      db.prepare(
        `INSERT INTO playlist_entries (entry_id, playlist_id, item_id, position, section)
         VALUES (?, ?, ?, ?, NULL)`,
      ).run(newId(), to, itemId, next)
    }

    let result
    db.transaction(() => {
      if (verb === 'move') {
        db.prepare('UPDATE items SET playlist_id = ?, updated_at = ? WHERE id = ?').run(to, now, item.id)
        db.prepare('UPDATE playlist_entries SET playlist_id = ? WHERE item_id = ?').run(to, item.id)
        // Positions in the new home are whatever they were in the old one;
        // renumber so the arrangement stays a dense list.
        db.prepare(
          `UPDATE playlist_entries SET position = (
             SELECT COUNT(*) FROM playlist_entries x
              WHERE x.playlist_id = playlist_entries.playlist_id
                AND (x.position < playlist_entries.position
                     OR (x.position = playlist_entries.position AND x.entry_id < playlist_entries.entry_id))
           ) WHERE playlist_id = ?`,
        ).run(to)
        result = db.prepare('SELECT * FROM items WHERE id = ?').get(item.id)
      } else {
        const copy = newId()
        db.prepare(
          `INSERT INTO items (id, playlist_id, take_id, label, template_id, template_rev, data,
                              destination, created_at, updated_at)
           SELECT ?, ?, NULL, label, template_id, template_rev, data, destination, ?, ?
             FROM items WHERE id = ?`,
          // take_id is the integrator-visible alias and it is UNIQUE. A copy is
          // a different Item and must not answer to the original's name.
        ).run(copy, to, now, now, item.id)
        db.prepare(
          `INSERT INTO item_destinations (item_id, ordinal, node_id, output, layer, template_rev,
                                          mode, blank, screen_id)
           SELECT ?, ordinal, node_id, output, layer, template_rev, mode, blank, screen_id
             FROM item_destinations WHERE item_id = ?`,
        ).run(copy, item.id)
        append(copy)
        result = db.prepare('SELECT * FROM items WHERE id = ?').get(copy)
      }
    })()

    reofferAll()
    res.status(verb === 'copy' ? 201 : 200).json(hydrate(result, db))
  })
}

router.delete('/api/items/:id', mayEdit, (req, res) => {
  const info = handle().prepare('DELETE FROM items WHERE id = ?').run(req.params.id)
  if (!info.changes) return fail(res, 404, 'not_found', 'There is no item with that id.')
  reofferAll()
  res.status(204).end()
})

/**
 * `POST /api/templates/:id/update-items` — move every Item pinned to an older
 * revision of THIS template onto `to` (default: head).
 *
 * Lives in the items router because it is an item mutation wearing a template
 * URL, and the validation it must reuse lives here.
 *
 * # Only this template's pins move
 *
 * An Item's destinations can pin DIFFERENT templates — the same words go to a
 * lower third on lt, a full-screen variant on fs, a wall look on wall. Bumping
 * every destination to `to` would quietly re-point the full-screen feed at the
 * lower-third template; it validates (the fields are a subset) and renders the
 * wrong graphic on Sunday. Exactly that mistake was made by hand, twice, in
 * rehearsal — this endpoint exists so it cannot happen again.
 *
 * # Per-item verdicts, not all-or-nothing
 *
 * Twenty lyric slides update; the one that fills a field the new revision
 * dropped is REFUSED with a sentence and stays pinned where it renders.
 * Failing all twenty because of it would turn a safe operation into an
 * either/or nobody needs; updating it anyway would break a graphic silently.
 */
router.post('/api/templates/:id/update-items', mayEdit, (req, res) => {
  const db = handle()
  const t = db.prepare('SELECT * FROM templates WHERE id = ?').get(req.params.id)
  if (!t) return fail(res, 404, 'not_found', 'There is no template with that id.')
  const to = req.body?.to ?? t.head_rev
  const target = revision(db, to)
  if (!target || target.template_id !== t.id) {
    return fail(res, 422, 'validation_failed', 'That revision does not belong to this template.')
  }

  // Which template a pinned revision belongs to, cached — the loop below asks
  // per destination.
  const ownerOf = new Map()
  const owner = (rev) => {
    if (!ownerOf.has(rev)) {
      ownerOf.set(
        rev,
        db.prepare('SELECT template_id FROM template_revisions WHERE template_rev = ?').get(rev)
          ?.template_id ?? null,
      )
    }
    return ownerOf.get(rev)
  }

  const candidates = db
    .prepare(
      `SELECT DISTINCT i.* FROM items i
        LEFT JOIN item_destinations d ON d.item_id = i.id
        WHERE (i.template_id = @tid AND i.template_rev != @to)
           OR (d.template_rev != @to AND d.template_rev IN
                (SELECT template_rev FROM template_revisions WHERE template_id = @tid))`,
    )
    .all({ tid: t.id, to })

  const updated = []
  const refused = []
  for (const cur of candidates) {
    const data = JSON.parse(cur.data)
    const dests = destinationsOf(db, cur.id).map((d) => ({
      ...d,
      blank: d.blank ? 1 : 0,
      template_rev: owner(d.template_rev) === t.id ? to : d.template_rev,
    }))
    const problems = validateDestinations(db, dests, data)
    if (problems.length) {
      refused.push({ id: cur.id, label: cur.label, problem: problems[0] })
      continue
    }
    const rev = cur.template_id === t.id ? to : cur.template_rev
    db.prepare('UPDATE items SET template_rev = ?, updated_at = ? WHERE id = ?').run(
      rev,
      Date.now(),
      cur.id,
    )
    writeDestinations(db, cur.id, dests)
    updated.push({ id: cur.id, label: cur.label })
  }
  if (updated.length) reofferAll()
  res.json({ to, updated, refused })
})
