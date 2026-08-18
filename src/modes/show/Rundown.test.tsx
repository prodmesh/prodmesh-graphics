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
  playlists: [{ id: 'p1', name: 'Welcome', mode: 'cue', items: [item('i1', 'Welcome'), item('i2', 'Pastor Dan')] }],
  cursor: { rundown: 'rd1', item: 'i2', index: 1, total: 2, label: 'Pastor Dan', next: null, previous: 'i1' },
}

/** The fixture's single playlist, so tests can vary its playout mode. */
const setMode = (mode: string) => {
  const p = RUNDOWN.playlists[0]
  if (p) p.mode = mode
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
  setMode('cue')
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

describe('playout mode — ADR 0017', () => {
  /** Re-serve the rundown with a different playlist mode. */
  const withMode = (mode: string) => {
    setMode(mode)
    const calls: { url: string; body: unknown }[] = []
    vi.stubGlobal('fetch', vi.fn(async (url: string, init?: RequestInit) => {
      calls.push({ url, body: init?.body ? JSON.parse(String(init.body)) : null })
      return new Response(JSON.stringify(RUNDOWN), { status: 200 })
    }))
    return calls
  }

  test('cue mode says the arrows move, and they only move', async () => {
    const calls = withMode('cue')
    await paint()

    expect(screen.getByText(/Cue — arrows move/)).toBeDefined()
    const panel = screen.getByRole('region', { name: 'Rundown' })
    await act(async () => {
      panel.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowUp', bubbles: true }))
      await Promise.resolve()
    })

    // The cursor moved and nothing fired. The arm step is the whole reason
    // this mode exists, and the stated objection to ProPresenter.
    expect(calls.some((c) => c.url.includes('/cursor'))).toBe(true)
    expect(calls.some((c) => c.url.includes('/api/cue'))).toBe(false)
  })

  test('follow mode takes on navigation, and takes the slide it is moving TO', async () => {
    const calls = withMode('follow')
    await paint()

    expect(screen.getByText(/Follow — arrows take/)).toBeDefined()
    const panel = screen.getByRole('region', { name: 'Rundown' })
    await act(async () => {
      panel.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowUp', bubbles: true }))
      await Promise.resolve()
    })

    const cue = calls.find((c) => c.url.includes('/api/cue'))
    expect(cue).toBeDefined()
    // The cursor is on i2; ArrowUp goes to i1, so i1 is what goes on air.
    // Firing what the cursor was ON would put the operator a slide behind the
    // band, which in a chorus is the entire problem.
    expect((cue!.body as { item: string }).item).toBe('i1')
  })

  test('an unknown mode is refused loudly and the keyboard goes quiet', async () => {
    const calls = withMode('timed-auto-advance')
    await paint()

    // Refused, not guessed at. Falling back to cue would give the operator an
    // arm step in a playlist authored to fire on navigation, with nothing on
    // screen saying so.
    const alert = await screen.findByRole('alert')
    expect(alert.textContent).toContain('timed-auto-advance')

    const panel = screen.getByRole('region', { name: 'Rundown' })
    await act(async () => {
      panel.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true }))
      await Promise.resolve()
    })
    expect(calls.some((c) => c.url.includes('/api/cue'))).toBe(false)
    expect(calls.some((c) => c.url.includes('/cursor'))).toBe(false)
  })
})

describe('jump to a section — the rundown calls this required', () => {
  const withSections = () => {
    RUNDOWN.playlists[0].mode = 'follow'
    RUNDOWN.playlists[0].items = [
      { ...item('i1', 'Welcome'), section: 'Verse 1' },
      { ...item('i2', 'Pastor Dan'), section: 'Chorus 1' },
      { ...item('i3', 'Third'), section: 'Chorus 1' },
    ]
    const calls: { url: string; body: unknown }[] = []
    vi.stubGlobal('fetch', vi.fn(async (url: string, init?: RequestInit) => {
      calls.push({ url, body: init?.body ? JSON.parse(String(init.body)) : null })
      return new Response(JSON.stringify(RUNDOWN), { status: 200 })
    }))
    return calls
  }

  test('each section appears once, in running order', async () => {
    withSections()
    await paint()
    const map = screen.getByRole('group', { name: 'Sections' })
    // Chorus 1 holds two slides and is ONE target. A map with a button per
    // slide is the list it was meant to replace.
    expect(map.textContent).toBe('Verse 1Chorus 1')
  })

  test('clicking one jumps the cursor by NAME, and in follow mode takes', async () => {
    const calls = withSections()
    await paint()
    await act(async () => {
      screen.getByRole('button', { name: 'Chorus 1' }).click()
      await Promise.resolve()
    })

    const cursor = calls.find((c) => c.url.includes('/cursor'))
    expect(cursor).toBeDefined()
    // By name, so the server resolves it to the section's FIRST entry — "go
    // back to the chorus" means start it again, not resume where it was left.
    expect((cursor!.body as { section: string }).section).toBe('Chorus 1')

    // And it fires, because in Follow navigating IS taking. A jump that only
    // moved the cursor would leave the previous slide on air while the operator
    // believed they had moved.
    const cue = calls.find((c) => c.url.includes('/api/cue'))
    expect(cue).toBeDefined()
    expect((cue!.body as { item: string }).item).toBe('i2')
  })
})
