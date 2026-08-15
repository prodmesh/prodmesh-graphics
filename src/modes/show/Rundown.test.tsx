/**
 * The rundown panel's two marks, and the one sentence.
 *
 * The rule that needed a test is the warning's condition. The first version
 * fired whenever the cursor was not what was on air — which, because Take next
 * fires and then advances, is nearly always. A warning on every take is one an
 * operator learns to ignore, so it may as well not exist.
 */

import { afterEach, beforeEach, describe, expect, test, vi } from 'vitest'
import { act, render, screen, waitFor } from '@testing-library/react'
import { RundownPanel } from './Rundown'
import { _reset as resetQuery } from '../../lib/useQuery'
import { _reset as resetStream } from '../../lib/stream'

const item = (id: string, label: string) => ({
  id, take_id: null, label, template_id: 't1', template_rev: 'blake3:aa',
  data: {}, destination: { layer: 'Lower Third' }, updated_at: 0,
})

const RUNDOWN = {
  id: 'rd1',
  name: 'Sunday',
  playlists: [{ id: 'p1', name: 'Welcome', items: [item('i1', 'Welcome'), item('i2', 'Pastor Dan')] }],
  cursor: { rundown: 'rd1', item: 'i2', index: 1, total: 2, label: 'Pastor Dan', next: null, previous: 'i1' },
}

let topicValues: Record<string, unknown> = {}

class FakeEventSource {
  listeners = new Map<string, ((e: MessageEvent) => void)[]>()
  constructor(readonly url: string) {
    queueMicrotask(() => {
      const topics = new URLSearchParams(url.split('?')[1]).get('topics')?.split(',') ?? []
      for (const t of topics) if (t in topicValues) this.emit('msg', { topic: t, data: topicValues[t] })
    })
  }
  addEventListener(type: string, fn: (e: MessageEvent) => void) {
    this.listeners.set(type, [...(this.listeners.get(type) ?? []), fn])
  }
  emit(type: string, payload: unknown) {
    for (const fn of this.listeners.get(type) ?? []) fn({ data: JSON.stringify(payload) } as MessageEvent)
  }
  close() {}
}

const layersWith = (occupants: { item_id: string; item_label: string }[]) => ({
  output: 'out-0', node: 'booth-1', fps: 59.94, late_frames: 0,
  layers: occupants.map((o, i) => ({
    name: `L${i}`,
    occupant: { ...o, template_rev: 'r', phase: 'hold', timeline: null, t_ms: 0, degraded: false },
  })),
})

beforeEach(() => {
  resetQuery()
  resetStream()
  topicValues = {}
  vi.stubGlobal('EventSource', FakeEventSource)
  vi.stubGlobal('fetch', vi.fn(async () => new Response(JSON.stringify(RUNDOWN), { status: 200 })))
})
afterEach(() => vi.unstubAllGlobals())

const paint = async () => {
  render(<RundownPanel rundownId="rd1" nodeId="booth-1" outputId="out-0" reachable show="default" />)
  await screen.findByText('Pastor Dan')
  await act(async () => { await Promise.resolve() })
}

describe('the divergence warning', () => {
  test('stays quiet when the cursor is simply ahead of the output', async () => {
    // The normal state after Take next: it fired i1 and advanced to i2.
    topicValues['output:out-0:layers'] = layersWith([{ item_id: 'i1', item_label: 'Welcome' }])
    await paint()
    await waitFor(() => expect(screen.getByText(/on air/)).toBeDefined())
    expect(screen.queryByRole('status')).toBeNull()
  })

  test('speaks up when the output shows something this rundown does not contain', async () => {
    topicValues['output:out-0:layers'] = layersWith([{ item_id: 'i9', item_label: 'LIVE bug' }])
    await paint()
    const note = await screen.findByRole('status')
    // Named, so the operator knows what to look at rather than being told
    // vaguely that something is wrong.
    expect(note.textContent).toContain('LIVE bug')
  })

  test('says nothing when nothing is on air', async () => {
    topicValues['output:out-0:layers'] = layersWith([])
    await paint()
    expect(screen.queryByRole('status')).toBeNull()
  })
})

describe('the two marks', () => {
  test('the cursor and what is on air are drawn separately', async () => {
    topicValues['output:out-0:layers'] = layersWith([{ item_id: 'i1', item_label: 'Welcome' }])
    await paint()
    await waitFor(() => {
      const cursor = document.querySelector('.rundown__item--cursor')
      const live = document.querySelector('.rundown__item--live')
      expect(cursor?.textContent).toContain('Pastor Dan')
      expect(live?.textContent).toContain('Welcome')
      // Different rows. Collapsing them into one mark would hide exactly the
      // state the panel exists to show.
      expect(cursor).not.toBe(live)
    })
  })
})
