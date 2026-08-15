import { test, after } from 'node:test'
import assert from 'node:assert/strict'
import { createApp } from './index.js'

const server = createApp().listen(0)
const base = () => `http://127.0.0.1:${server.address().port}`
after(() => server.close())

test('health reports the milestone and the stream registry', async () => {
  const r = await fetch(`${base()}/api/health`)
  assert.equal(r.status, 200)
  const body = await r.json()
  assert.equal(body.ok, true)
  assert.ok(body.stream.registeredPatterns.includes('show:*:cuelog'))
})

test('public health leaks nothing a wall display should not show', async () => {
  const body = await (await fetch(`${base()}/api/health/public`)).json()
  for (const c of body.components) {
    // Built from a fixed field list, so a new field upstream cannot put a host,
    // a port, or an error string on a screen in a lobby.
    assert.deepEqual(Object.keys(c).toSorted(), ['key', 'label', 'state'])
  }
})

test('version advertises the contracts a client has to match', async () => {
  const body = await (await fetch(`${base()}/api/version`)).json()
  assert.equal(typeof body.proto_ver, 'number')
  assert.equal(typeof body.scene_reader_major, 'number')
})

test('an invalid topic is dropped individually, not fatally', async () => {
  // One stale widget in a saved layout must not blank the other eleven.
  const ctrl = new AbortController()
  const r = await fetch(`${base()}/api/stream?topics=nonsense:topic`, { signal: ctrl.signal })
  assert.equal(r.status, 200)
  assert.match(r.headers.get('content-type'), /text\/event-stream/)
  ctrl.abort()
})
