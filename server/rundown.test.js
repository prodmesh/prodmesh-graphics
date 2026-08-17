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
  d.exec(`DELETE FROM rundown_cursor; DELETE FROM rundown_playlists; DELETE FROM playlist_items;
          DELETE FROM rundowns; DELETE FROM playlists; DELETE FROM items;
          DELETE FROM templates; DELETE FROM template_revisions;`)
  d.prepare(`INSERT INTO templates (id,name,created_at,updated_at) VALUES ('t1','T',0,0)`).run()
  d.prepare(
    `INSERT INTO template_revisions (template_rev,template_id,name,min_reader,manifest,published_at)
     VALUES (?, 't1','T',1,'{"fields":[]}',0)`,
  ).run(REV)
  for (const [id, label] of [['i1', 'Welcome'], ['i2', 'Pastor Dan'], ['i3', 'Closing']]) {
    d.prepare(
      `INSERT INTO items (id,label,template_id,template_rev,data,destination,created_at,updated_at)
       VALUES (?, ?, 't1', ?, '{}', '{"layer":"Lower Third"}', 0, 0)`,
    ).run(id, label, REV)
  }
}

beforeEach(seed)

async function service() {
  const pl = (await req('POST', '/api/playlists', { name: 'Welcome' })).json
  await req('PUT', `/api/playlists/${pl.id}/items`, { items: ['i1', 'i2'] })
  const pl2 = (await req('POST', '/api/playlists', { name: 'Message' })).json
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

test('the same item twice in one playlist is refused', async () => {
  const { pl } = await service()
  const r = await req('PUT', `/api/playlists/${pl.id}/items`, { items: ['i1', 'i1'] })
  assert.equal(r.status, 422)
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
