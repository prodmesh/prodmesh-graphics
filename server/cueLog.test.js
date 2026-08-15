import { test, beforeEach } from 'node:test'
import assert from 'node:assert/strict'
import { append, since, _reset } from './cueLog.js'

beforeEach(() => _reset())

const entry = (over = {}) => ({
  cue_id: '01JA', surface: 'show_view', op: 'take',
  layer: 'Lower Third', item_label: 'Pastor Dan', ...over,
})

test('sequence numbers are monotonic and start at 1', () => {
  assert.equal(append('s1', entry()).seq, 1)
  assert.equal(append('s1', entry()).seq, 2)
})

test('a surface is required — an unknown one is a bug, not a default', () => {
  // Without `surface`, the log is confusing the first time someone uses a GPI.
  assert.throws(() => append('s1', entry({ surface: 'mystery' })))
  assert.throws(() => append('s1', entry({ surface: undefined })))
})

test('?since=<seq> returns exactly the gap — no duplicates, no holes', () => {
  for (let i = 0; i < 10; i++) append('s1', entry())
  const gap = since('s1', 4)
  assert.equal(gap.head_seq, 10)
  assert.deepEqual(gap.entries.map((e) => e.seq), [5, 6, 7, 8, 9, 10])
})

test('a client that never left asks since=head and gets nothing', () => {
  for (let i = 0; i < 3; i++) append('s1', entry())
  assert.deepEqual(since('s1', 3).entries, [])
})

test('logs are per show', () => {
  append('s1', entry())
  append('s2', entry())
  assert.equal(since('s1', 0).head_seq, 1)
  assert.equal(since('s2', 0).head_seq, 1)
})

test('fired_at_frame is distinct from at_utc — when it went on, not when asked', () => {
  const e = append('s1', entry({ fired_at_frame: 1841944 }))
  assert.equal(e.fired_at_frame, 1841944)
  assert.notEqual(e.fired_at_frame, e.at_utc)
  // Absent an ack it is explicitly null rather than quietly zero.
  assert.equal(append('s1', entry()).fired_at_frame, null)
})
