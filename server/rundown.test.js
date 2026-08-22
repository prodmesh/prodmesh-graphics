/**
 * Playlists, the rundown, and the cursor.
 *
 * The claim worth defending: **the cursor is intent, not fact.** Moving it puts
 * nothing on air, and it is allowed to disagree with what a node reports —
 * because an operator can take from the rack, a GPI can fire at the node, and a
 * cue can be rejected. A cursor that silently re-fired to stay in agreement
 * would be the worst of all worlds.
 */

import assert from 'node:assert/strict'
import test, { before, beforeEach } from 'node:test'
import fs from 'node:fs'

let rundown, db, app, port, server

before(async () => {
  process.env.PMG_DB = ':memory:'
  process.env.PMG_CAS = fs.mkdtempSync('/tmp/pmg-rundown-cas-')
  rundown = await import('./rundown.js')
  db = await import('./db.js')
  app = (await import('./index.js')).createApp()
  server = app.listen(0)
  await new Promise((r) => server.once('listening', r))
  port = server.address().port
})

test.after(() => server?.close())

const req = async (method, url, body) => {
  const res = await fetch(`http://127.0.0.1:${port}${url}`, {
    method,
    headers: body ? { 'content-type': 'application/json' } : {},
    ...(body ? { body: JSON.stringify(body) } : {}),
  })
  const text = await res.text()
  return { status: res.status, json: text ? JSON.parse(text) : null }
}

const REV = `blake3:${'0'.repeat(64)}`

/** Three items, so an order is actually observable. */
function seed() {
  const d = db.handle()
  d.exec(`DELETE FROM rundown_cursor; DELETE FROM rundown_playlists; DELETE FROM playlist_entries;
          DELETE FROM rundowns; DELETE FROM playlists; DELETE FROM items;
          DELETE FROM templates; DELETE FROM template_revisions;`)
  d.prepare(`INSERT INTO templates (id,name,created_at,updated_at) VALUES ('t1','T',0,0)`).run()
  d.prepare(
    `INSERT INTO template_revisions (template_rev,template_id,name,min_reader,manifest,published_at)
     VALUES (?, 't1','T',1,'{"fields":[]}',0)`,
  ).run(REV)
  // Items belong to a playlist now (ADR 0026); the seed makes the two homes
  // the service below arranges from.
  d.prepare(`INSERT INTO playlists (id,name,mode,created_at,updated_at) VALUES ('p1','Welcome','cue',0,0)`).run()
  d.prepare(`INSERT INTO playlists (id,name,mode,created_at,updated_at) VALUES ('p2','Message','cue',0,0)`).run()
  for (const [id, label, home] of [
    ['i1', 'Welcome', 'p1'],
    ['i2', 'Pastor Dan', 'p1'],
    ['i3', 'Closing', 'p2'],
  ]) {
    d.prepare(
      `INSERT INTO items (id,label,template_id,template_rev,data,destination,created_at,updated_at,playlist_id)
       VALUES (?, ?, 't1', ?, '{}', '{"layer":"Lower Third"}', 0, 0, ?)`,
    ).run(id, label, REV, home)
  }
}

beforeEach(seed)

async function service() {
  const pl = { id: 'p1' }
  const pl2 = { id: 'p2' }
  await req('PUT', `/api/playlists/${pl.id}/items`, { items: ['i1', 'i2'] })
  await req('PUT', `/api/playlists/${pl2.id}/items`, { items: ['i3'] })
  const rd = (await req('POST', '/api/rundowns', { name: 'Sunday 09:00' })).json
  await req('PUT', `/api/rundowns/${rd.id}/playlists`, { playlists: [pl.id, pl2.id] })
  return { rd, pl, pl2 }
}

test('a rundown flattens to one running order across its playlists', async () => {
  const { rd } = await service()
  const order = rundown.order(rd.id)
  assert.deepEqual(order.map((o) => o.label), ['Welcome', 'Pastor Dan', 'Closing'])
  assert.deepEqual(order.map((o) => o.playlist_name), ['Welcome', 'Welcome', 'Message'])
})

test('reordering replaces the whole list, densely', async () => {
  const { pl } = await service()
  const updated = (await req('PUT', `/api/playlists/${pl.id}/items`, { items: ['i2', 'i1'] })).json
  assert.deepEqual(updated.items.map((i) => i.id), ['i2', 'i1'])
  assert.deepEqual(updated.items.map((i) => i.position), [0, 1], 'positions stay dense')
})

test('the cursor starts at the top and advances without taking anything', async () => {
  const { rd } = await service()
  const start = (await req('GET', `/api/rundowns/${rd.id}/cursor`)).json
  assert.equal(start.label, 'Welcome')
  assert.equal(start.index, 0)
  assert.equal(start.total, 3)

  const next = (await req('PUT', `/api/rundowns/${rd.id}/cursor`, { delta: 1 })).json
  assert.equal(next.label, 'Pastor Dan')
  assert.equal(next.previous, 'i1')
  assert.equal(next.next, 'i3')

  // Nothing went on air. Advancing is navigation; taking is a cue, and fusing
  // them would make "I moved to the wrong place" indistinguishable from "I put
  // the wrong graphic up".
  const log = (await req('GET', '/api/cuelog?show=default&since=0')).json
  assert.equal(log.entries.length, 0)
})

test('the cursor clamps rather than wrapping', async () => {
  const { rd } = await service()
  const end = (await req('PUT', `/api/rundowns/${rd.id}/cursor`, { delta: 99 })).json
  assert.equal(end.label, 'Closing')
  assert.equal(end.next, null)
  // Falling off the end of a service and landing back at the welcome slide is
  // a worse surprise than a button that does nothing.
  const still = (await req('PUT', `/api/rundowns/${rd.id}/cursor`, { delta: 1 })).json
  assert.equal(still.label, 'Closing')

  const top = (await req('PUT', `/api/rundowns/${rd.id}/cursor`, { delta: -99 })).json
  assert.equal(top.label, 'Welcome')
  assert.equal(top.previous, null)
})

test('jumping to an item outside the rundown is refused', async () => {
  const { rd } = await service()
  const r = await req('PUT', `/api/rundowns/${rd.id}/cursor`, { item: 'i-not-here' })
  assert.equal(r.status, 422)
  assert.match(r.json.label, /not in this rundown/)
})

test('a playlist cannot reference an item that does not exist', async () => {
  const { pl } = await service()
  const r = await req('PUT', `/api/playlists/${pl.id}/items`, { items: ['i1', 'ghost'] })
  assert.equal(r.status, 422)
  // A rundown with a hole in it is discovered on Sunday. This is Thursday.
  assert.match(r.json.label, /not on this server/)
})

/**
 * The reverse of what this used to assert, and the reverse is the point
 * (ADR 0026): a chorus that comes round three times is ONE piece of content in
 * three places, so fixing a typo in it fixes all three.
 */
test('the same item twice in one playlist is the point, and each place is its own entry', async () => {
  const { pl, rd } = await service()
  const r = await req('PUT', `/api/playlists/${pl.id}/items`, { items: ['i1', 'i2', 'i1'] })
  assert.equal(r.status, 200)
  assert.deepEqual(r.json.items.map((i) => i.label), ['Welcome', 'Pastor Dan', 'Welcome'])
  const entries = r.json.items.map((i) => i.entry_id)
  assert.equal(new Set(entries).size, 3, 'three places, three entries')

  // And the cursor can tell them apart, which is the whole reason entries
  // exist: addressed by item, moving on from the second "Welcome" would jump
  // backwards to the first.
  const order = rundown.order(rd.id)
  const secondWelcome = order.findLastIndex((o) => o.id === 'i1')
  await req('PUT', `/api/rundowns/${rd.id}/cursor`, { entry: order[secondWelcome].entry_id })
  const c = (await req('GET', `/api/rundowns/${rd.id}/cursor`)).json
  assert.equal(c.index, secondWelcome)
  assert.equal(c.item, 'i1')
})

test('an item from another playlist cannot be arranged into this one', async () => {
  const { pl } = await service()
  // Two songs that both say "Verse 1" are two pieces of content; sharing one
  // between playlists is how a fix to tonight's set silently edits last week's.
  const r = await req('PUT', `/api/playlists/${pl.id}/items`, { items: ['i1', 'i3'] })
  assert.equal(r.status, 422)
  assert.match(r.json.label, /belongs to the playlist/)
})

test('deleting an item removes it from the running order', async () => {
  const { rd } = await service()
  db.handle().prepare('DELETE FROM items WHERE id = ?').run('i2')
  const order = rundown.order(rd.id)
  // ON DELETE CASCADE on playlist_items. Without it the rundown would keep a
  // row pointing at nothing and the cursor could land on it.
  assert.deepEqual(order.map((o) => o.label), ['Welcome', 'Closing'])
})

test('the cursor survives the item it points at being removed', async () => {
  const { rd } = await service()
  await req('PUT', `/api/rundowns/${rd.id}/cursor`, { item: 'i2' })
  db.handle().prepare('DELETE FROM items WHERE id = ?').run('i2')

  const c = (await req('GET', `/api/rundowns/${rd.id}/cursor`)).json
  // It reports honestly that it is nowhere (index -1) rather than silently
  // sliding to a neighbour, which would move an operator's place without
  // telling them.
  assert.equal(c.index, -1)
  assert.equal(c.total, 2)
})

// ---------------------------------------------------------------------------
// Playout mode — ADR 0017.
// ---------------------------------------------------------------------------

test('a playlist is created in cue mode unless it asks for otherwise', async () => {
  const pl = (await req('POST', '/api/playlists', { name: 'Message' })).json
  // Cue is the safe default: it has an arm step. A playlist that silently came
  // up in follow would put a graphic on air the first time someone clicked a
  // row to see what was in it.
  assert.equal(pl.mode, 'cue')

  const song = (await req('POST', '/api/playlists', { name: 'Song', mode: 'follow' })).json
  assert.equal(song.mode, 'follow')
})

test('mode is changed through its own endpoint, not as a field on a rename', async () => {
  const pl = (await req('POST', '/api/playlists', { name: 'Song' })).json
  const r = await req('PUT', `/api/playlists/${pl.id}/mode`, { mode: 'follow' })
  assert.equal(r.status, 200)
  assert.equal(r.json.mode, 'follow')
  assert.equal((await req('GET', `/api/playlists/${pl.id}`)).json.mode, 'follow')
})

test('an unknown mode is refused rather than stored', async () => {
  const pl = (await req('POST', '/api/playlists', { name: 'Song' })).json
  const r = await req('PUT', `/api/playlists/${pl.id}/mode`, { mode: 'whatever' })
  assert.equal(r.status, 422)
  // Names what IS allowed. "Invalid mode" tells an operator nothing they can
  // act on, and this endpoint is one a control surface will call.
  assert.match(r.json.label, /cue or follow/)
  assert.equal((await req('GET', `/api/playlists/${pl.id}`)).json.mode, 'cue', 'unchanged')
})

test('the mode travels with the rundown, because the keyboard depends on it', async () => {
  const { rd, pl2 } = await service()
  await req('PUT', `/api/playlists/${pl2.id}/mode`, { mode: 'follow' })
  const detail = (await req('GET', `/api/rundowns/${rd.id}`)).json
  // The browser decides what the arrow keys do from the playlist the cursor is
  // in, so the mode has to arrive with the running order rather than needing a
  // second fetch per playlist.
  // A rundown mixes both kinds — that is the normal Sunday — so what the
  // arrow keys do changes as the cursor crosses a boundary.
  assert.deepEqual(detail.playlists.map((p) => p.mode), ['cue', 'follow'])
})

// ---------------------------------------------------------------------------
// Sections, and jumping to one.
//
// The reference rundown calls this required, not a nicety: *"the worship leader
// suddenly decides to sing the Chorus again. Operator clicks the first slide in
// the Chorus to go back."* Under time pressure, with the band playing.
// ---------------------------------------------------------------------------

/** A song's own slides. An Item belongs to one playlist (ADR 0026). */
function song(name, labels) {
  const d = db.handle()
  const pl = d.prepare('SELECT id FROM playlists WHERE name = ?').get(name)
  const ids = []
  labels.forEach((label, i) => {
    const id = `${pl.id}-${i}`
    d.prepare(
      `INSERT INTO items (id,label,template_id,template_rev,data,destination,created_at,updated_at,playlist_id)
       VALUES (?, ?, 't1', ?, '{}', '{"layer":"Lower Third"}', 0, 0, ?)`,
    ).run(id, label, REV, pl.id)
    ids.push(id)
  })
  return { id: pl.id, items: ids }
}

test('a playlist entry can name its section, and a bare id list still works', async () => {
  const made = (await req('POST', '/api/playlists', { name: 'Song', mode: 'follow' })).json
  const pl = song('Song', ['Verse 1', 'Chorus', 'Bridge'])
  const [i1, i2, i3] = pl.items
  void made
  const r = await req('PUT', `/api/playlists/${pl.id}/items`, {
    items: [
      { item: i1, section: 'Verse 1' },
      { item: i2, section: 'Chorus 1' },
      i3,
    ],
  })
  assert.equal(r.status, 200)
  assert.deepEqual(r.json.items.map((i) => i.section), ['Verse 1', 'Chorus 1', null])
})

test('jumping to a section lands on its first entry', async () => {
  await req('POST', '/api/playlists', { name: 'Song', mode: 'follow' })
  const pl = song('Song', ['Verse 1', 'Chorus', 'Chorus again'])
  const [i1, i2, i3] = pl.items
  await req('PUT', `/api/playlists/${pl.id}/items`, {
    items: [
      { item: i1, section: 'Verse 1' },
      { item: i2, section: 'Chorus 1' },
      { item: i3, section: 'Chorus 1' },
    ],
  })
  const rd = (await req('POST', '/api/rundowns', { name: 'Sunday' })).json
  await req('PUT', `/api/rundowns/${rd.id}/playlists`, { playlists: [pl.id] })

  const r = await req('PUT', `/api/rundowns/${rd.id}/cursor`, { section: 'Chorus 1' })
  assert.equal(r.status, 200)
  // The FIRST slide of the chorus, not the last one sung. "Go back to the
  // chorus" means start it again.
  assert.equal(r.json.item, i2)
})

test('a section this rundown does not have is refused by name', async () => {
  const { rd } = await service()
  const r = await req('PUT', `/api/rundowns/${rd.id}/cursor`, { section: 'Bridge 2' })
  assert.equal(r.status, 422)
  // Named, so an operator can see it was a typo rather than a broken button.
  assert.match(r.json.label, /Bridge 2/)
})

// ---------------------------------------------------------------------------
// Naming and unmaking, which the rail could not do at all

test('a rundown and a playlist can be renamed', async () => {
  seed()
  const r = await req('POST', '/api/rundowns', { name: 'Sunday' })
  const renamed = await req('PATCH', `/api/rundowns/${r.json.id}`, { name: 'Christmas Eve' })
  assert.equal(renamed.status, 200)
  assert.equal(renamed.json.name, 'Christmas Eve')

  const p = await req('POST', '/api/playlists', { name: 'Playlist 5' })
  const pr = await req('PATCH', `/api/playlists/${p.json.id}`, { name: 'Amazing Grace' })
  assert.equal(pr.json.name, 'Amazing Grace')

  assert.equal((await req('PATCH', `/api/rundowns/${r.json.id}`, { name: '  ' })).status, 422)
  assert.equal((await req('PATCH', '/api/rundowns/nope', { name: 'x' })).status, 404)
})

test('deleting a rundown leaves its playlists alone', async () => {
  seed()
  const p = await req('POST', '/api/playlists', { name: 'Amazing Grace' })
  const r = await req('POST', '/api/rundowns', { name: 'Sunday' })
  await req('PUT', `/api/rundowns/${r.json.id}/playlists`, { playlists: [p.json.id] })

  assert.equal((await req('DELETE', `/api/rundowns/${r.json.id}`)).status, 204)
  assert.equal((await req('GET', `/api/rundowns/${r.json.id}`)).status, 404)
  // A rundown is an ORDER over playlists. Deleting Sunday's running order must
  // not delete Sunday's content.
  assert.equal((await req('GET', `/api/playlists/${p.json.id}`)).status, 200)

  assert.equal((await req('DELETE', '/api/rundowns/nope')).status, 404)
})

// ---------------------------------------------------------------------------
// The library: ownership, copies, moves, tags. ADR 0026.

test('an item is created into a playlist, and lands in its arrangement', async () => {
  seed()
  const r = await req('POST', '/api/items', {
    playlist_id: 'p1',
    template_id: 't1',
    template_rev: REV,
    label: 'Verse 2',
    data: {},
    destinations: [{ output: 'lt', layer: 'Lower Third', template_rev: REV, mode: 'auto' }],
  })
  assert.equal(r.status, 201, r.json?.label ?? '')
  const pl = (await req('GET', '/api/playlists/p1')).json
  // An Item that exists but is nowhere in the order is one somebody has to go
  // and find.
  assert.ok(pl.items.some((i) => i.label === 'Verse 2'))
})

test('an item created into nowhere is refused', async () => {
  seed()
  const r = await req('POST', '/api/items', {
    template_id: 't1',
    template_rev: REV,
    label: 'Homeless',
    data: {},
    destinations: [{ output: 'lt', layer: 'Lower Third', template_rev: REV, mode: 'auto' }],
  })
  assert.equal(r.status, 422)
  assert.match(r.json.label, /belongs to a playlist/)
})

test('copying an item makes a SECOND item, and editing one leaves the other alone', async () => {
  seed()
  const copy = await req('POST', '/api/items/i1/copy', { playlist_id: 'p2' })
  assert.equal(copy.status, 201)
  assert.notEqual(copy.json.id, 'i1')

  await req('PATCH', `/api/items/${copy.json.id}`, { label: 'Welcome (wide)' })
  // Two songs that both say "Verse 1" are two pieces of content. A shared
  // instance across playlists is how a fix to tonight's set silently edits
  // last week's.
  assert.equal((await req('GET', '/api/items/i1')).json.label, 'Welcome')

  // And it is in the target's arrangement, not floating.
  assert.ok((await req('GET', '/api/playlists/p2')).json.items.some((i) => i.id === copy.json.id))
})

test('moving an item takes its places in the arrangement with it', async () => {
  seed()
  await req('PUT', '/api/playlists/p1/items', { items: ['i1', 'i2'] })
  const moved = await req('POST', '/api/items/i2/move', { playlist_id: 'p2' })
  assert.equal(moved.status, 200)

  const from = (await req('GET', '/api/playlists/p1')).json
  const to = (await req('GET', '/api/playlists/p2')).json
  // Leaving entries behind would leave the old playlist pointing at content it
  // no longer owns.
  assert.deepEqual(from.items.map((i) => i.id), ['i1'])
  assert.ok(to.items.some((i) => i.id === 'i2'))
  assert.deepEqual(to.items.map((i) => i.position), [...to.items.keys()])

  assert.equal((await req('POST', '/api/items/i2/move', { playlist_id: 'p2' })).status, 422)
})

test('tags are free strings on a playlist, and every one typed is offered back', async () => {
  seed()
  const r = await req('PUT', '/api/playlists/p1/tags', { tags: ['Christmas', ' Hymn ', 'Christmas', ''] })
  assert.equal(r.status, 200)
  // Trimmed, de-duplicated, and blanks dropped: a tag list that accumulates
  // "Hymn" and "Hymn " is a filter that quietly misses half its playlists.
  assert.deepEqual(r.json.tags, ['Christmas', 'Hymn'])

  await req('PUT', '/api/playlists/p2/tags', { tags: ['Hymn'] })
  assert.deepEqual((await req('GET', '/api/tags')).json.tags, [
    { tag: 'Christmas', playlists: 1 },
    { tag: 'Hymn', playlists: 2 },
  ])

  const lists = (await req('GET', '/api/playlists')).json.playlists
  assert.deepEqual(lists.find((p) => p.id === 'p1').tags, ['Christmas', 'Hymn'])
})

test('search finds an item by its label and by what was typed into it', async () => {
  seed()
  db.handle()
    .prepare("UPDATE items SET data = '{\"line\":\"Amazing grace how sweet the sound\"}' WHERE id = 'i2'")
    .run()

  const byLabel = (await req('GET', '/api/items?q=closing')).json.items
  assert.deepEqual(byLabel.map((i) => i.id), ['i3'])

  // The question "which song has that line?" is the one a search has to answer;
  // it is why "All items" could go.
  const byText = (await req('GET', '/api/items?q=sweet%20the%20sound')).json.items
  assert.deepEqual(byText.map((i) => i.id), ['i2'])
  assert.equal(byText[0].playlist_name, 'Welcome')

  assert.deepEqual((await req('GET', '/api/items?playlist=p2')).json.items.map((i) => i.id), ['i3'])
})

// ---------------------------------------------------------------------------
// Named arrangements. ADR 0027.

/** A song with real groups: a four-slide verse and a two-slide chorus. */
async function grouped() {
  seed()
  await req('POST', '/api/playlists', { name: 'Song', mode: 'follow' })
  const pl = song('Song', ['V1a', 'V1b', 'C1a', 'C1b', 'V2a'])
  const [a, b, c, d, e] = pl.items
  await req('PUT', `/api/playlists/${pl.id}/items`, {
    items: [
      { item: a, section: 'Verse 1' },
      { item: b, section: 'Verse 1' },
      { item: c, section: 'Chorus' },
      { item: d, section: 'Chorus' },
      { item: e, section: 'Verse 2' },
    ],
  })
  return pl
}

test('a group is the FIRST run of a section name, so a repeat in Main is not more slides', async () => {
  const pl = await grouped()
  const [a, b, c, d, e] = pl.items
  // Main sings the chorus twice. The group is still two slides — folding both
  // runs together would make every arrangement sing a four-slide chorus.
  await req('PUT', `/api/playlists/${pl.id}/items`, {
    items: [
      { item: a, section: 'Verse 1' },
      { item: b, section: 'Verse 1' },
      { item: c, section: 'Chorus' },
      { item: d, section: 'Chorus' },
      { item: e, section: 'Verse 2' },
      { item: c, section: 'Chorus' },
      { item: d, section: 'Chorus' },
    ],
  })
  const groups = rundown.groupsOf(pl.id)
  assert.deepEqual(groups.map((g) => [g.name, g.entries.length]), [
    ['Verse 1', 2],
    ['Chorus', 2],
    ['Verse 2', 1],
  ])
})

test('an arrangement is an order of group NAMES, and expands to Main’s slides', async () => {
  const pl = await grouped()
  const made = await req('POST', `/api/playlists/${pl.id}/arrangements`, { name: 'Short' })
  assert.equal(made.status, 201)
  // It starts as Main's groups in Main's order — an empty arrangement is a song
  // that plays nothing, and nobody means that.
  assert.deepEqual(made.json.groups, ['Verse 1', 'Chorus', 'Verse 2'])

  const set = await req('PUT', `/api/arrangements/${made.json.id}`, {
    groups: ['Chorus', 'Verse 1', 'Chorus'],
  })
  assert.equal(set.status, 200)

  const under = (await req('GET', `/api/playlists/${pl.id}?arrangement=${made.json.id}`)).json
  assert.deepEqual(under.items.map((i) => i.label), ['C1a', 'C1b', 'V1a', 'V1b', 'C1a', 'C1b'])
  // Each occurrence is its own place, or the cursor could not tell the two
  // choruses apart (ADR 0026).
  assert.equal(new Set(under.items.map((i) => i.entry_id)).size, 6)

  // And Main is untouched: it is the order that owns the slides.
  assert.deepEqual((await req('GET', `/api/playlists/${pl.id}`)).json.items.map((i) => i.label), [
    'V1a', 'V1b', 'C1a', 'C1b', 'V2a',
  ])
})

test('editing Main flows into every arrangement using that group', async () => {
  const pl = await grouped()
  const made = (await req('POST', `/api/playlists/${pl.id}/arrangements`, { name: 'Short' })).json
  await req('PUT', `/api/arrangements/${made.id}`, { groups: ['Chorus'] })

  await req('PATCH', `/api/items/${pl.items[2]}`, { label: 'C1a fixed' })
  const under = (await req('GET', `/api/playlists/${pl.id}?arrangement=${made.id}`)).json
  // An arrangement that stored SLIDES would still be singing the old words.
  assert.deepEqual(under.items.map((i) => i.label), ['C1a fixed', 'C1b'])
})

test('a group Main does not have is refused, rather than expanding to a gap', async () => {
  const pl = await grouped()
  const made = (await req('POST', `/api/playlists/${pl.id}/arrangements`, { name: 'Short' })).json
  const r = await req('PUT', `/api/arrangements/${made.id}`, { groups: ['Chorus', 'Rap solo'] })
  assert.equal(r.status, 422)
  assert.match(r.json.label, /no group called/)
})

test('a playlist with no groups cannot have an arrangement, and is told why', async () => {
  seed()
  const r = await req('POST', '/api/playlists/p2/arrangements', { name: 'Short' })
  assert.equal(r.status, 422)
  assert.match(r.json.label, /Name the groups in Main first/)
})

test('a rundown pins the arrangement it wants of a song', async () => {
  const pl = await grouped()
  const made = (await req('POST', `/api/playlists/${pl.id}/arrangements`, { name: 'Short' })).json
  await req('PUT', `/api/arrangements/${made.id}`, { groups: ['Chorus'] })

  const rd = (await req('POST', '/api/rundowns', { name: 'Sunday 09:00' })).json
  await req('PUT', `/api/rundowns/${rd.id}/playlists`, {
    playlists: [{ playlist: pl.id, arrangement: made.id }],
  })
  assert.deepEqual(rundown.order(rd.id).map((o) => o.label), ['C1a', 'C1b'])

  // The same song, the other service, the full order.
  const rd2 = (await req('POST', '/api/rundowns', { name: 'Sunday 11:00' })).json
  await req('PUT', `/api/rundowns/${rd2.id}/playlists`, { playlists: [pl.id] })
  assert.equal(rundown.order(rd2.id).length, 5)

  // Deleting the arrangement drops the pin to Main rather than breaking the
  // running order.
  assert.equal((await req('DELETE', `/api/arrangements/${made.id}`)).status, 204)
  assert.equal(rundown.order(rd.id).length, 5)
})

test('an arrangement belonging to another song cannot be pinned', async () => {
  const pl = await grouped()
  const made = (await req('POST', `/api/playlists/${pl.id}/arrangements`, { name: 'Short' })).json
  const rd = (await req('POST', '/api/rundowns', { name: 'Sunday' })).json
  // p1 is a different playlist. Pinning another song's order would expand to
  // nothing on the morning it mattered.
  await req('PUT', `/api/rundowns/${rd.id}/playlists`, {
    playlists: [{ playlist: 'p1', arrangement: made.id }],
  })
  const r = (await req('GET', `/api/rundowns/${rd.id}`)).json
  assert.equal(r.playlists[0].arrangement, null)
})
