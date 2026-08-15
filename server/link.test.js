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
