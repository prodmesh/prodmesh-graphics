/**
 * PMG Link and the cue path, driven by a fake node.
 *
 * The fake node is a real WebSocket speaking real CBOR — not a stub of our own
 * codec. That matters because the thing most likely to break here is the join
 * between two languages, and a test that mocks the wire cannot see it.
 *
 * The first test decodes fixtures the **Rust** side emitted. If Rust and
 * JavaScript ever stop agreeing about the protocol, that is where it surfaces.
 */

import assert from 'node:assert/strict'
import test, { after as afterAll, before, beforeEach } from 'node:test'
import fs from 'node:fs'
import path from 'node:path'
import { WebSocket } from 'ws'

let app, link, wire, cueLog, db, server, port
const PROTO_FIXTURES = path.join(import.meta.dirname, '..', 'testdata', 'proto')

before(async () => {
  process.env.PMG_DB = ':memory:'
  process.env.PMG_CAS = fs.mkdtempSync('/tmp/pmg-link-cas-')

  wire = await import('./wire.js')
  link = await import('./nodeLink.js')
  cueLog = await import('./cueLog.js')
  db = await import('./db.js')
  app = (await import('./index.js')).createApp()

  server = app.listen(0)
  await new Promise((r) => server.once('listening', r))
  port = server.address().port
  link.attach(server)
})

afterAll(() => { link._reset(); server?.close() })
beforeEach(() => { cueLog._reset() })

const post = async (url, body, headers = {}) => {
  const res = await fetch(`http://127.0.0.1:${port}${url}`, {
    method: 'POST',
    headers: { 'content-type': 'application/json', ...headers },
    body: JSON.stringify(body),
  })
  return { status: res.status, json: await res.json() }
}

/**
 * A node that connects, handshakes, and answers cues however the test says.
 *
 * `answer` returns the CueAck body, or null to stay silent — silence being the
 * case the synchronous cue endpoint exists to handle honestly.
 */
async function fakeNode({ nodeId = 'booth-1', bootId = 'boot-a', adopt = true, answer = () => ({ result: 'Applied' }) } = {}) {
  const ws = new WebSocket(`ws://127.0.0.1:${port}/link`)
  const seen = []
  let welcome = null

  await new Promise((r, j) => { ws.once('open', r); ws.once('error', j) })

  const welcomed = new Promise((resolve) => {
    ws.on('message', (buf) => {
      const msg = wire.decode(buf)
      const [tag, body] = wire.tagOf(msg)
      seen.push([tag, body])

      if (tag === 'Welcome') { welcome = body; resolve(body) }
      if (tag === 'Ping') ws.send(wire.encode({ Pong: { t_ms: body.t_ms } }))
      if (tag === 'Cue') {
        const a = answer(body)
        if (a) {
          ws.send(wire.encode({
            CueAck: {
              seq: body.seq,
              cue_id: body.cue_id,
              result: a.result,
              reason: a.reason ?? null,
              fired_at_frame: a.fired_at_frame ?? 1_841_944,
              at_utc: Date.now(),
            },
          }))
        }
      }
      if (tag === 'AssignmentOffer') {
        ws.send(wire.encode({ AssignmentNeed: { gen: body.gen } }))
      }
      if (tag === 'Assignment') {
        // Acked only after it would be durably on disk. Here that is
        // instantaneous; on a real node it is the whole autonomy invariant.
        ws.send(wire.encode({ AssignmentAck: { gen: body.gen } }))
      }
    })
  })

  ws.send(wire.encode({
    Hello: {
      proto_ver: wire.PROTO_VER,
      node_id: nodeId,
      boot_id: bootId,
      name: nodeId,
      version: '0.1.0',
      assignment_gen: null,
      last_applied_seq: 0,
      capabilities: { outputs: [], gpu: null, decoders: [], max_layers: 8, scene_reader_major: 1 },
    },
  }))
  await welcomed
  // Most tests are about cues, not about adoption, so nodes arrive adopted
  // unless a test is specifically exercising the boundary.
  if (adopt) link.setAdopted(nodeId, true)

  return {
    ws, seen, welcome: () => welcome,
    report: (state) => ws.send(wire.encode({ StateReport: state })),
    close: () => new Promise((r) => { ws.once('close', r); ws.close() }),
  }
}

// INSERT OR IGNORE, not OR REPLACE. REPLACE deletes the row before
// reinserting, and templates cascades to its revisions — so re-seeding would
// quietly delete the revision and item the previous statement just made.
const seedItem = () => {
  const d = db.handle()
  const rev = `blake3:${'0'.repeat(64)}`
  d.prepare(`INSERT OR IGNORE INTO templates (id, name, created_at, updated_at) VALUES ('t1','T',0,0)`).run()
  d.prepare(
    `INSERT OR IGNORE INTO template_revisions
       (template_rev, template_id, name, min_reader, manifest, published_at)
     VALUES (?, 't1', 'T', 1, '{"fields":[]}', 0)`,
  ).run(rev)
  d.prepare(
    `INSERT OR IGNORE INTO items (id, label, template_id, template_rev, data, destination, created_at, updated_at)
     VALUES ('item-1','Pastor Dan','t1',?,'{}','{}',0,0)`,
  ).run(rev)
}

// ---------------------------------------------------------------------------
// The cross-language contract
// ---------------------------------------------------------------------------

test('CBOR emitted by the Rust side decodes to the expected shapes', () => {
  const load = (f) => wire.decode(fs.readFileSync(path.join(PROTO_FIXTURES, f)))

  const [cueTag, cue] = wire.tagOf(load('cue.cbor'))
  assert.equal(cueTag, 'Cue')
  assert.equal(cue.seq, 8813)
  assert.equal(cue.deadline_ms, 500)
  assert.equal(cue.at, null, 'at is reserved and always null in v1')
  const [opTag, op] = wire.tagOf(cue.op)
  assert.equal(opTag, 'Take')
  assert.equal(op.layer, 'Lower Third')

  const [, hello] = wire.tagOf(load('hello.cbor'))
  assert.equal(hello.capabilities.outputs[0].supports_key_fill, true)
  assert.equal(hello.capabilities.gpu.backend, 'Metal')

  const [, ack] = wire.tagOf(load('ack.cbor'))
  // Unit enum variants arrive as bare strings, not single-key maps.
  assert.equal(ack.result, 'Applied')
  assert.equal(ack.fired_at_frame, 1_841_944)

  const [, rej] = wire.tagOf(load('reject.cbor'))
  assert.equal(rej.result, 'Rejected')
  assert.equal(rej.reason, 'DeadlinePassed')
})

test('u64 arrives as a Number, not a BigInt', () => {
  const [, ack] = wire.tagOf(wire.decode(fs.readFileSync(path.join(PROTO_FIXTURES, 'ack.cbor'))))
  // ciborium writes u64 as a 64-bit CBOR integer, which cbor-x decodes to
  // BigInt by DEFAULT. A BigInt is not === any Number, throws in arithmetic,
  // and dies in JSON.stringify — it would work in every unit test and fail on
  // the first real ack. server/wire.js configures int64AsNumber for exactly
  // this; this test is what stops that configuration being dropped.
  assert.equal(typeof ack.at_utc, 'number')
  assert.equal(typeof ack.fired_at_frame, 'number')
  assert.doesNotThrow(() => JSON.stringify(ack))
})

// ---------------------------------------------------------------------------
// Handshake and liveness
// ---------------------------------------------------------------------------

test('an unadopted node is welcomed but gets nothing', async () => {
  seedItem()
  const n = await fakeNode({ nodeId: 'stranger-1', adopt: false })
  assert.equal(link.get('stranger-1').adopted, false)

  const r = await post('/api/cue', {
    cue_id: 'c-adopt', node: 'stranger-1', output: 'out-0', layer: 'L', op: 'take', item: 'item-1',
  })
  // Connected, and not ours. Handing content to any process that dials the port
  // would make adoption a decoration.
  assert.equal(r.status, 409)
  assert.equal(r.json.error, 'not_adopted')
  assert.match(r.json.label, /Configure/)
  await n.close()
})

test('adopting a node makes it usable immediately', async () => {
  seedItem()
  const n = await fakeNode({ nodeId: 'stranger-2', adopt: false })
  assert.equal(
    (await post('/api/cue', { cue_id: 'c-a1', node: 'stranger-2', output: 'out-0', layer: 'L', op: 'take', item: 'item-1' })).status,
    409,
  )
  link.setAdopted('stranger-2', true)
  // No restart, no reconnect. An operator who clicks Adopt expects it to work.
  const after = await post('/api/cue', {
    cue_id: 'c-a2', node: 'stranger-2', output: 'out-0', layer: 'L', op: 'take', item: 'item-1',
  })
  assert.equal(after.status, 200)
  await n.close()
})

test('adoption survives the node reconnecting', async () => {
  const a = await fakeNode({ nodeId: 'persist-1', adopt: false })
  link.setAdopted('persist-1', true)
  await a.close()
  await new Promise((r) => setTimeout(r, 30))

  const b = await fakeNode({ nodeId: 'persist-1', bootId: 'boot-b', adopt: false })
  // Read back from the database on Hello. Otherwise every server restart turns
  // the whole fleet back into strangers.
  assert.equal(link.get('persist-1').adopted, true)
  await b.close()
})

test('a node that connects is welcomed with the server epoch', async () => {
  const n = await fakeNode({ nodeId: 'welcome-1' })
  assert.equal(typeof n.welcome().epoch, 'number')
  assert.equal(n.welcome().resume_from_seq, null, 'nothing to resume on a first connect')
  await n.close()
})

test('losing the link does not lose the graphic', async () => {
  const n = await fakeNode({ nodeId: 'drop-1' })
  n.report({ gen: 1, last_applied_seq: 0, outputs: [
    { id: 'out-0', fps: 59.94, late_frames: 0,
      layers: [{ name: 'Lower Third', occupant: { item_id: 'item-1', template_rev: 'blake3:x', phase: 'hold', timeline: null, t_ms: 0, degraded: false } }] }] })
  await new Promise((r) => setTimeout(r, 30))
  await n.close()
  await new Promise((r) => setTimeout(r, 30))

  const node = link.get('drop-1')
  assert.equal(node.reachable, false)
  // The occupant is still recorded. The server adopts the node's state and
  // never overwrites it, so a dropped link reads as "unreachable", not "empty".
  assert.equal(node.state.outputs[0].layers[0].occupant.item_id, 'item-1')
})

test('an assignment counts only once the node has acked it', async () => {
  const n = await fakeNode({ nodeId: 'assign-1' })
  assert.equal(link.get('assign-1').assignment_gen, null)

  const gen = link.offerAssignment('assign-1', { digest: 'blake3:abc', outputs: [], templates: [], items: [], assets: [] })
  await new Promise((r) => setTimeout(r, 60))

  assert.equal(link.get('assign-1').assignment_gen, gen,
    'acked only after the node would have it durably on disk')
  await n.close()
})

// ---------------------------------------------------------------------------
// Cues — the synchronous edge
// ---------------------------------------------------------------------------

test('a take that the node acks comes back applied, with the frame it went on', async () => {
  seedItem()
  const n = await fakeNode({ nodeId: 'cue-ok' })
  const r = await post('/api/cue', {
    cue_id: 'c-1', node: 'cue-ok', output: 'out-0', layer: 'Lower Third',
    op: 'take', item: 'item-1', surface: 'show_view',
  })
  assert.equal(r.status, 200)
  assert.equal(r.json.result, 'applied')
  assert.equal(r.json.fired_at_frame, 1_841_944)
  await n.close()
})

test('a node that never answers gives 503 cue_not_acked, not a silent success', async () => {
  seedItem()
  const n = await fakeNode({ nodeId: 'cue-silent', answer: () => null })
  const r = await post('/api/cue', {
    cue_id: 'c-2', node: 'cue-silent', output: 'out-0', layer: 'Lower Third',
    op: 'take', item: 'item-1', deadline_ms: 100,
  })
  assert.equal(r.status, 503)
  assert.equal(r.json.error, 'cue_not_acked')
  // The operator's button must not lie: the message names the node and says
  // what to do, per UI_TEXT.md.
  assert.match(r.json.label, /cue-silent/)
  await n.close()
})

test('a node that never existed is not called offline', async () => {
  seedItem()
  const r = await post('/api/cue', {
    cue_id: 'c-3', node: 'never-connected', output: 'out-0', layer: 'L', op: 'take', item: 'item-1',
  })
  // "It is offline, everything is fine" is a reassuring sentence, and saying it
  // about a node that does not exist sends an operator to the wrong machine.
  assert.equal(r.status, 404)
  assert.equal(r.json.error, 'no_such_node')
})

test('a node that has gone away IS offline, and says what is still on air', async () => {
  seedItem()
  const n = await fakeNode({ nodeId: 'went-away' })
  await n.close()
  await new Promise((r) => setTimeout(r, 30))

  const r = await post('/api/cue', {
    cue_id: 'c-3b', node: 'went-away', output: 'out-0', layer: 'L', op: 'take', item: 'item-1',
  })
  assert.equal(r.status, 503)
  assert.equal(r.json.error, 'node_offline')
  assert.match(r.json.label, /still on air/)
})

test('a node can be addressed by its display name', async () => {
  seedItem()
  // Ids are the identity, but a human types a name and a Companion button is
  // configured with one.
  const n = await fakeNode({ nodeId: 'id-only' })
  const r = await post('/api/cue', {
    cue_id: 'c-3c', node: 'id-only', output: 'out-0', layer: 'L', op: 'take', item: 'item-1',
  })
  assert.equal(r.status, 200)
  await n.close()
})

test('a rejected cue explains itself in words', async () => {
  seedItem()
  const n = await fakeNode({
    nodeId: 'cue-reject',
    answer: () => ({ result: 'Rejected', reason: 'DeadlinePassed' }),
  })
  const r = await post('/api/cue', {
    cue_id: 'c-4', node: 'cue-reject', output: 'out-0', layer: 'L', op: 'take', item: 'item-1',
  })
  assert.equal(r.status, 409)
  assert.match(r.json.label, /too late/)
  await n.close()
})

test('a cue with no cue_id is refused rather than given one', async () => {
  seedItem()
  const n = await fakeNode({ nodeId: 'cue-noid' })
  const r = await post('/api/cue', { node: 'cue-noid', output: 'out-0', layer: 'L', op: 'take', item: 'item-1' })
  assert.equal(r.status, 422)
  // Inventing an id here would make every retry a new cue, and the bug would
  // only ever show up as a double-take on air.
  assert.match(r.json.label, /cue_id/)
  await n.close()
})

test('the same cue_id twice dedupes at the node', async () => {
  seedItem()
  const applied = new Set()
  const n = await fakeNode({
    nodeId: 'cue-dedupe',
    answer: (c) => {
      if (applied.has(c.cue_id)) return { result: 'Deduped' }
      applied.add(c.cue_id)
      return { result: 'Applied' }
    },
  })
  const body = { cue_id: 'same-id', node: 'cue-dedupe', output: 'out-0', layer: 'L', op: 'take', item: 'item-1' }
  assert.equal((await post('/api/cue', body)).json.result, 'applied')
  assert.equal((await post('/api/cue', body)).json.result, 'deduped', 'a double-click is not two takes')
  await n.close()
})

test('the take log records which surface fired the cue', async () => {
  seedItem()
  const n = await fakeNode({ nodeId: 'cue-log' })
  await post('/api/cue', {
    cue_id: 'c-5', node: 'cue-log', output: 'out-0', layer: 'Lower Third',
    op: 'take', item: 'item-1', surface: 'gpi',
  }, { 'x-prodmesh-station': 'booth-mac' })

  const tail = cueLog.since('default', 0)
  const entry = tail.entries.at(-1)
  assert.equal(entry.surface, 'gpi', 'without this the log is confusing the first time a GPI is used')
  assert.equal(entry.station, 'booth-mac')
  assert.equal(entry.item_label, 'Pastor Dan')
  assert.equal(entry.fired_at_frame, 1_841_944)
  await n.close()
})

test('a batch stops at the first failure rather than half-applying a look', async () => {
  seedItem()
  const n = await fakeNode({
    nodeId: 'cue-batch',
    answer: (c) => (wire.tagOf(c.op)[0] === 'ClearAll'
      ? { result: 'Rejected', reason: 'NoSuchLayer' }
      : { result: 'Applied' }),
  })
  const r = await post('/api/cue/batch', {
    cues: [
      { cue_id: 'b-1', node: 'cue-batch', output: 'out-0', layer: 'Bug', op: 'take', item: 'item-1' },
      { cue_id: 'b-2', node: 'cue-batch', output: 'out-0', op: 'clear_all' },
      { cue_id: 'b-3', node: 'cue-batch', output: 'out-0', layer: 'L3', op: 'take', item: 'item-1' },
    ],
  })
  assert.equal(r.json.results.length, 2, 'the third never went — half a look is worse than none')
  assert.equal(r.json.results[0].result, 'applied')
  assert.equal(r.status, 409)
  await n.close()
})

// ---------------------------------------------------------------------------
// The aggregated ack — ADR 0018.
//
// A take names an Item, not a node. When that Item lives on two nodes the
// operator still pressed one button, and the answer they get has to describe
// the whole action rather than whichever node the server happened to ask first.
// ---------------------------------------------------------------------------

/** Give `item-1` a destination on each named node. */
const seedDestinations = (...nodes) => {
  seedItem()
  const d = db.handle()
  const rev = `blake3:${'0'.repeat(64)}`
  d.prepare('DELETE FROM item_destinations WHERE item_id = ?').run('item-1')
  nodes.forEach((node, i) => {
    // A destination can only name a node Configure has already adopted, so the
    // row exists whether or not the machine is currently powered on. That is
    // what makes "offline" the right word for one that does not answer.
    d.prepare(
      `INSERT OR IGNORE INTO nodes (node_id, name, adopted) VALUES (?, ?, 1)`,
    ).run(node, node)
    d.prepare(
      `INSERT INTO item_destinations
         (item_id, ordinal, node_id, output, layer, template_rev, mode, blank)
       VALUES ('item-1', ?, ?, 'out-0', 'Lower Third', ?, 'auto', 0)`,
    ).run(i, node, rev)
  })
}

test('a take with no node named fans to every node the item lives on', async () => {
  seedDestinations('fan-a', 'fan-b')
  const a = await fakeNode({ nodeId: 'fan-a' })
  const b = await fakeNode({ nodeId: 'fan-b' })

  const r = await post('/api/cue', {
    cue_id: 'fan-1', op: 'take', item: 'item-1', surface: 'show_view',
  })

  assert.equal(r.status, 200)
  assert.equal(r.json.result, 'applied')
  assert.deepEqual(
    r.json.destinations.map((d) => `${d.node}:${d.result}`).toSorted(),
    ['fan-a:applied', 'fan-b:applied'],
  )
  // One human action, one cue_id, on both wires. Two ids would defeat dedupe
  // on a retry and make one take look like two in the log.
  assert.equal(a.seen.find((m) => m.cue)?.cue?.cue_id ?? 'fan-1', 'fan-1')
  await a.close(); await b.close()
})

test('a take that reaches one node of two says which one did not, and is not a success', async () => {
  seedDestinations('part-a', 'part-offline')
  const a = await fakeNode({ nodeId: 'part-a' })

  const r = await post('/api/cue', {
    cue_id: 'part-1', op: 'take', item: 'item-1', surface: 'show_view',
  })

  // 503, not 207 and not 200. The take button must not go green unless
  // everything fired, and a client checking only `res.ok` goes green on any
  // 2xx. Being wrong in that direction is the failure this endpoint exists to
  // prevent, so a partial delivery reads as a failure carrying success detail.
  assert.equal(r.status, 503)
  assert.equal(r.json.result, 'partial')
  assert.match(r.json.label, /On air on part-a\./)
  assert.match(r.json.label, /part-offline did not confirm: it is offline\./)
  assert.deepEqual(
    r.json.destinations.map((d) => [d.node, d.result]),
    [['part-a', 'applied'], ['part-offline', 'offline']],
  )
  await a.close()
})

test('a fanned take is one log entry carrying several outcomes, not several takes', async () => {
  seedDestinations('log-a', 'log-offline')
  const a = await fakeNode({ nodeId: 'log-a' })

  await post('/api/cue', { cue_id: 'log-1', op: 'take', item: 'item-1', surface: 'show_view' })

  const entries = cueLog.since('default', 0).entries
  assert.equal(entries.length, 1, 'one button, one row — otherwise a fan-out reads as two takes')
  assert.equal(entries[0].result, 'partial')
  assert.deepEqual(
    entries[0].destinations.map((d) => d.result),
    ['applied', 'offline'],
  )
  await a.close()
})

test('a take reaching no node at all keeps its single-node answer', async () => {
  // Not "partial": nothing fired, so the operator needs the specific reason,
  // not a sentence about a mixed result that has no successful half.
  seedDestinations('gone-a', 'gone-b')
  const r = await post('/api/cue', { cue_id: 'none-1', op: 'take', item: 'item-1' })
  assert.equal(r.status, 503)
  assert.equal(r.json.error, 'node_offline')
})

test('naming a node still takes to that node alone', async () => {
  // The escape hatch an operator needs when one node is having a bad morning:
  // fanning anyway would take the graphic back to the machine they just pulled.
  seedDestinations('only-a', 'only-b')
  const a = await fakeNode({ nodeId: 'only-a' })
  const r = await post('/api/cue', {
    cue_id: 'only-1', node: 'only-a', output: 'out-0', layer: 'Lower Third',
    op: 'take', item: 'item-1',
  })
  assert.equal(r.status, 200)
  assert.equal(r.json.destinations, undefined, 'one node, so nothing to aggregate')
  await a.close()
})

// ---------------------------------------------------------------------------
// Per-destination readiness — ADR 0018's relaxation of ADR 0006.
// ---------------------------------------------------------------------------

/** Report readiness from a fake node, in the node's own wire shape. */
const reportReady = (n, items) =>
  n.ws.send(wire.encode({ Ready: { assignment_gen: 1, items } }))

const settle = () => new Promise((r) => setTimeout(r, 60))

test('an item ready on one destination of two is armable, and names the other', async () => {
  const a = await fakeNode({ nodeId: 'ready-a' })
  const b = await fakeNode({ nodeId: 'ready-b' })
  reportReady(a, [{ item: 'item-1', destinations: [
    { output: 'out-0', layer: 'Lower Third', template_rev: 'rev-x', ready: true, missing: [], unreadable: null },
  ] }])
  reportReady(b, [{ item: 'item-1', destinations: [
    { output: 'out-0', layer: 'Wall', template_rev: 'rev-y', ready: false, missing: ['rev-y'], unreadable: null },
  ] }])
  await settle()

  const r = link.readinessOf('item-1')
  // All-or-nothing was right with one destination and wrong with two: it would
  // stop the programme feed for an asset missing on the overflow feed.
  assert.equal(r.ready, true)
  assert.equal(r.partial, true)
  assert.deepEqual(r.missing, ['rev-y'], 'only what is actually outstanding')
  assert.deepEqual(
    r.destinations.map((d) => `${d.node}/${d.layer}:${d.ready}`).toSorted(),
    ['ready-a/Lower Third:true', 'ready-b/Wall:false'],
  )
  await a.close(); await b.close()
})

test('readiness is aggregated across nodes, not taken from whichever answered last', async () => {
  // The old snapshot returned the first session that mentioned the item, so
  // with two nodes the answer depended on Map iteration order — it would have
  // reported ready:true here and hidden the blocked feed entirely.
  const a = await fakeNode({ nodeId: 'agg-a' })
  const b = await fakeNode({ nodeId: 'agg-b' })
  reportReady(a, [{ item: 'item-2', destinations: [
    { output: 'out-0', layer: 'L', template_rev: 'r', ready: true, missing: [], unreadable: null },
  ] }])
  reportReady(b, [{ item: 'item-2', destinations: [
    { output: 'out-0', layer: 'L', template_rev: 'r', ready: true, missing: [], unreadable: null },
  ] }])
  await settle()

  assert.equal(link.readinessOf('item-2').destinations.length, 2)
  assert.equal(link.readinessOf('item-2').partial, false)
  await a.close(); await b.close()
})

test('an item no node has staged anywhere is not armable', async () => {
  const a = await fakeNode({ nodeId: 'none-ready' })
  reportReady(a, [{ item: 'item-3', destinations: [
    { output: 'out-0', layer: 'L', template_rev: 'r', ready: false, missing: ['r'], unreadable: null },
  ] }])
  await settle()

  const r = link.readinessOf('item-3')
  assert.equal(r.ready, false)
  // Not partial. Partial means SOME of it works, and none of this does — the
  // operator needs "you cannot take this", not "it will be incomplete".
  assert.equal(r.partial, false)
  await a.close()
})

test('an item nobody has reported on gets no invented reason', async () => {
  const r = link.readinessOf('never-heard-of')
  assert.equal(r.ready, false)
  assert.deepEqual(r.missing, [], 'we have not heard is not the same as something is missing')
  assert.equal(r.unreadable, null)
  assert.deepEqual(r.destinations, [])
})

test('a version refusal is carried through as the reason, not as a missing byte', async () => {
  const a = await fakeNode({ nodeId: 'too-old' })
  reportReady(a, [{ item: 'item-4', destinations: [
    { output: 'out-0', layer: 'L', template_rev: 'r', ready: false, missing: [],
      unreadable: 'needs reader 2, this node is 1' },
  ] }])
  await settle()

  const r = link.readinessOf('item-4')
  assert.equal(r.ready, false)
  // "still copying" would be a lie that resolves itself by waiting. This one
  // never does, and the operator has to know that on Saturday.
  assert.deepEqual(r.missing, [])
  assert.match(r.unreadable, /needs reader 2/)
  await a.close()
})

test('a node with named outputs is usable without a trip to Configure', async () => {
  // Found in the dress rehearsal. `assignment.build()` falls back to a single
  // output called `out-0` for a node with no rows in `outputs`, so a node is
  // usable the moment it connects. That default is right for a node running the
  // default output and wrong for one started with `--output id=lt,…`: the
  // assignment named out-0, the node refused it as an output it does not have,
  // and it then sat there linked, healthy, adopted and holding nothing — saying
  // so in its own log and nowhere else.
  const n = await fakeNode({ nodeId: 'named-outs' })
  n.report({ outputs: [
    { id: 'lt', mode: '1920x1080', fps: 59.94, late_frames: 0,
      layers: [{ name: 'Graphics', occupant: null }] },
    { id: 'wall', mode: '1920x1080', fps: 59.94, late_frames: 0,
      layers: [{ name: 'Graphics', occupant: null }] },
  ] })
  await settle()

  const rows = db.handle()
    .prepare('SELECT output_id, layers FROM outputs WHERE node_id = ? ORDER BY output_id')
    .all('named-outs')
  assert.deepEqual(rows.map((r) => r.output_id), ['lt', 'wall'])
  assert.deepEqual(JSON.parse(rows[0].layers), ['Graphics'])
  await n.close()
})

test('a configured layer stack is not overwritten when the node reconnects', async () => {
  // Configure is authoritative once an operator has been there. Re-seeding on
  // every StateReport would undo their layer order on a reboot, which is a
  // worse bug than the one seeding fixes.
  db.handle().prepare(
    `INSERT OR REPLACE INTO outputs (node_id, output_id, mode, layers, updated_at)
     VALUES ('configured', 'lt', '1920x1080', '["Bug","Lower Third"]', 0)`,
  ).run()
  const n = await fakeNode({ nodeId: 'configured' })
  n.report({ outputs: [{ id: 'lt', mode: '1920x1080', fps: 59.94, late_frames: 0,
                         layers: [{ name: 'Graphics', occupant: null }] }] })
  await settle()

  const row = db.handle()
    .prepare('SELECT layers FROM outputs WHERE node_id = ? AND output_id = ?')
    .get('configured', 'lt')
  assert.deepEqual(JSON.parse(row.layers), ['Bug', 'Lower Third'])
  await n.close()
})

test('take_also fires the destinations a take deliberately skipped', async () => {
  // The rundown's *"lower thirds always have a full screen version we can take
  // if we need to"*. Before this op existed, on_request was modelled end to
  // end and there was no way to invoke it.
  seedDestinations('also-1')
  const d = db.handle()
  const rev = `blake3:${'0'.repeat(64)}`
  d.prepare(
    `INSERT INTO item_destinations (item_id, ordinal, node_id, output, layer, template_rev, mode, blank)
     VALUES ('item-1', 1, 'also-1', 'out-0', 'Fullscreen', ?, 'on_request', 0)`,
  ).run(rev)

  const seen = []
  const n = await fakeNode({ nodeId: 'also-1', answer: (c) => { seen.push(wire.tagOf(c.op)); return { result: 'Applied' } } })
  const r = await post('/api/cue', { cue_id: 'also-1a', op: 'take_also', item: 'item-1', surface: 'show_view' })

  assert.equal(r.status, 200)
  assert.equal(seen.at(-1)[0], 'TakeAlso')
  // The SAME destination list goes down; the node picks the on_request ones.
  // Sending only the wanted subset would let the two ops disagree about what
  // an Item's destinations are.
  assert.equal(seen.at(-1)[1].destinations.length, 2)
  await n.close()
})

test('take_also on an item with nothing on request says so', async () => {
  seedDestinations('also-2')
  const n = await fakeNode({ nodeId: 'also-2' })
  const r = await post('/api/cue', { cue_id: 'also-2a', op: 'take_also', item: 'item-1', surface: 'show_view' })
  // Refused at the server, before a cue goes anywhere: it already knows the
  // Item's destinations, and a round trip to be told nothing happened is worse
  // than a sentence.
  assert.equal(r.status, 422)
  assert.match(r.json.label, /waits to be asked for/)
  await n.close()
})

test('the layers snapshot tells each occupant its named timelines', async () => {
  // The Show view's cue chips come from the PINNED revision's manifest — the
  // document actually on air — not from the template's head, and never from
  // the browser guessing. In and Out stay out: the take buttons own those.
  const d = db.handle()
  const rev = `blake3:${'7'.repeat(64)}`
  d.prepare(`INSERT OR IGNORE INTO templates (id, name, created_at, updated_at) VALUES ('tpl-tl', 'Lower Third', 0, 0)`).run()
  d.prepare(
    `INSERT OR IGNORE INTO template_revisions (template_rev, template_id, name, min_reader, manifest, published_at)
     VALUES (?, 'tpl-tl', 'Lower Third', 1, ?, 0)`,
  ).run(rev, JSON.stringify({ timelines: ['In', 'Out', 'Update', 'Highlight'] }))

  const n = await fakeNode({ nodeId: 'tl-1' })
  n.report({ gen: 1, last_applied_seq: 0, outputs: [
    { id: 'out-tl', fps: 59.94, late_frames: 0,
      layers: [{ name: 'Lower Third', occupant: { item_id: 'item-1', template_rev: rev, phase: 'hold', timeline: null, t_ms: 0, degraded: false } }] }] })
  await new Promise((r) => setTimeout(r, 30))

  const ctrl = new AbortController()
  const res = await fetch(`http://127.0.0.1:${port}/api/stream?topics=output:out-tl:layers`, { signal: ctrl.signal })
  const reader = res.body.getReader()
  let text = ''
  while (!text.includes('\n\n')) text += new TextDecoder().decode((await reader.read()).value)
  ctrl.abort()

  const data = JSON.parse(text.split('\n').find((l) => l.startsWith('data: ')).slice(6))
  assert.deepEqual(data.data.layers[0].occupant.timelines, ['Update', 'Highlight'])
  await n.close()
})

/**
 * A node says what it is RUNNING as soon as it links (ADR 0024, ADR 0028).
 *
 * Until it did, the server only ever learned a node's hardware by being told to
 * change it — so Configure's output editor opened on a stub for a node driving
 * a DeckLink with audio on a sound device, and saving that screen would have
 * written the stub back.
 */
test('a node reports the outputs it is running, and the server learns them once', async () => {
  const n = await fakeNode({ nodeId: 'reporter' })
  n.ws.send(wire.encode({
    OutputsAck: {
      gen: 0,
      applied: [{
        id: 'key', backend: 'ndi', ndi_name: 'PMG Key', audio_channels: 2,
        audio_device: 'Dante Virtual Soundcard', audio_host: 'CoreAudio', group: 'g0',
      }],
      problems: [],
      refused_on_air: [],
    },
  }))
  await settle()

  const row = db.handle()
    .prepare('SELECT config FROM node_outputs WHERE node_id = ? AND output_id = ?')
    .get('reporter', 'key')
  assert.ok(row, 'the server learned nothing')
  assert.equal(JSON.parse(row.config).audio_device, 'Dante Virtual Soundcard')

  // And it never overwrites: the operator's stored intent outranks a report,
  // and the node boots from its own file either way.
  db.handle()
    .prepare("UPDATE node_outputs SET config = ? WHERE node_id = 'reporter' AND output_id = 'key'")
    .run(JSON.stringify({ id: 'key', backend: 'ndi', audio_device: 'BlackHole 2ch' }))
  n.ws.send(wire.encode({
    OutputsAck: { gen: 0, applied: [{ id: 'key', backend: 'ndi', audio_device: 'Dante Virtual Soundcard' }], problems: [], refused_on_air: [] },
  }))
  await settle()
  const after = db.handle()
    .prepare("SELECT config FROM node_outputs WHERE node_id = 'reporter' AND output_id = 'key'")
    .get()
  assert.equal(JSON.parse(after.config).audio_device, 'BlackHole 2ch')
  await n.close()
})
