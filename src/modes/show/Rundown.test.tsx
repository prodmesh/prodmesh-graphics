/**
 * The rundown console's two marks, the mode rules, and the one sentence.
 *
 * Rendered through Show with a rundown present, because that is how the
 * operator meets these rules. The claims are ADR 0017's: the cursor and what
 * is on air are two different marks; the keyboard is a function of the focused
 * playlist's mode; an unknown mode is refused loudly; and the divergence
 * warning only fires for something this rundown does not contain — a warning
 * on every take is one an operator learns to ignore.
 */

import { afterEach, beforeEach, describe, expect, test, vi } from 'vitest'
import { act, render, screen, waitFor, within } from '@testing-library/react'
import { Show } from './Show'
import { _reset as resetQuery } from '../../lib/useQuery'
import { _reset as resetStream } from '../../lib/stream'

const NODE = {
  node_id: 'booth-1',
  name: 'Booth-1',
  reachable: true,
  version: '0.1.0',
  assignment_gen: 1,
  last_seen: Date.now(),
  state: { outputs: [{ id: 'out-0' }] },
}

/**
 * An Item at a PLACE in a playlist. The entry is what identifies the place —
 * the same Item may sit in several (ADR 0026) — so the fixture carries one and
 * a test that matched on the item id alone cannot pass by accident.
 */
const item = (id: string, label: string, section: string | null = null, entry = `e-${id}`) => ({
  id, take_id: null, label, template_id: 't1', template_rev: 'blake3:aa',
  data: {}, destination: { layer: 'Lower Third' }, section, updated_at: 0, entry_id: entry,
})

const RUNDOWN = {
  id: 'rd1',
  name: 'Sunday',
  playlists: [{ id: 'p1', name: 'Opening', mode: 'cue', items: [item('i1', 'Welcome'), item('i2', 'Pastor Dan')] }],
  cursor: {
    rundown: 'rd1', item: 'i2', entry: 'e-i2', index: 1, total: 2, label: 'Pastor Dan',
    next: null, previous: 'i1', next_entry: null, previous_entry: 'e-i1',
  },
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

/** Serve the whole console: node, items, rundown list, rundown detail, cue. */
const serve = () => {
  const calls: { url: string; body: unknown }[] = []
  vi.stubGlobal('fetch', vi.fn(async (url: string, init?: RequestInit) => {
    calls.push({ url: String(url), body: init?.body ? JSON.parse(String(init.body)) : null })
    const body =
      String(url) === '/api/nodes' ? { nodes: [NODE] }
      : String(url) === '/api/items' ? { items: [] }
      : String(url) === '/api/rundowns' ? { rundowns: [{ id: 'rd1', name: 'Sunday' }] }
      : String(url).startsWith('/api/rundowns/rd1') ? RUNDOWN
      : String(url).startsWith('/api/cue') ? { result: 'applied' }
      : {}
    return new Response(JSON.stringify(body), { status: 200 })
  }))
  return calls
}

beforeEach(() => {
  resetQuery()
  resetStream()
  topicValues = {}
  vi.stubGlobal('EventSource', FakeEventSource)
  vi.stubGlobal('crypto', { ...globalThis.crypto, randomUUID: () => 'cue-fixed' })
  setMode('cue')
})
afterEach(() => vi.unstubAllGlobals())

const paint = async () => {
  render(<Show />)
  await screen.findAllByText('Pastor Dan')
  await act(async () => { await Promise.resolve() })
}

describe('the divergence warning', () => {
  test('stays quiet when the cursor is simply ahead of the output', async () => {
    serve()
    // The normal state after Take next: it fired i1 and advanced to i2.
    topicValues['output:out-0:layers'] = layersWith([{ item_id: 'i1', item_label: 'Welcome' }])
    await paint()
    await waitFor(() => expect(screen.getAllByText(/ON AIR/).length).toBeGreaterThan(0))
    expect(screen.queryByRole('status')).toBeNull()
  })

  test('speaks up when the output shows something this rundown does not contain', async () => {
    serve()
    topicValues['output:out-0:layers'] = layersWith([{ item_id: 'i9', item_label: 'LIVE bug' }])
    await paint()
    const note = await screen.findByRole('status')
    // Named, so the operator knows what to look at rather than being told
    // vaguely that something is wrong.
    expect(note.textContent).toContain('LIVE bug')
  })

  test('says nothing when nothing is on air', async () => {
    serve()
    topicValues['output:out-0:layers'] = layersWith([])
    await paint()
    expect(screen.queryByRole('status')).toBeNull()
  })
})

describe('the two marks', () => {
  test('the cursor and what is on air are drawn separately', async () => {
    serve()
    topicValues['output:out-0:layers'] = layersWith([{ item_id: 'i1', item_label: 'Welcome' }])
    await paint()
    await waitFor(() => {
      const rail = screen.getByRole('region', { name: 'Rundown' })
      const cursor = rail.querySelector('.c-rundownitem--armed')
      const live = rail.querySelector('.c-rundownitem--air')
      expect(cursor?.textContent).toContain('Pastor Dan')
      expect(live?.textContent).toContain('Welcome')
      // Different rows. Collapsing them into one mark would hide exactly the
      // state the panel exists to show.
      expect(cursor).not.toBe(live)
    })
  })
})

describe('playout mode — ADR 0017', () => {
  test('cue mode says the arrows move, and they only move', async () => {
    setMode('cue')
    const calls = serve()
    await paint()

    expect(screen.getByText(/CUE · ARM ONLY/)).toBeDefined()
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
    setMode('follow')
    const calls = serve()
    await paint()

    expect(screen.getByText(/FOLLOW · TAKE ON NAV/)).toBeDefined()
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

  test('clicking a row in follow mode fires it — navigating IS taking', async () => {
    setMode('follow')
    const calls = serve()
    await paint()

    const rail = screen.getByRole('region', { name: 'Rundown' })
    await act(async () => {
      within(rail).getByText('Welcome').click()
      await Promise.resolve()
    })

    const cue = calls.find((c) => c.url.includes('/api/cue'))
    expect(cue).toBeDefined()
    expect((cue!.body as { item: string }).item).toBe('i1')
  })

  test('clicking a row in cue mode only moves the cursor', async () => {
    setMode('cue')
    const calls = serve()
    await paint()

    const rail = screen.getByRole('region', { name: 'Rundown' })
    await act(async () => {
      within(rail).getByText('Welcome').click()
      await Promise.resolve()
    })

    expect(calls.some((c) => c.url.includes('/cursor'))).toBe(true)
    expect(calls.some((c) => c.url.includes('/api/cue'))).toBe(false)
  })

  test('an unknown mode is refused loudly and the keyboard goes quiet', async () => {
    setMode('timed-auto-advance')
    const calls = serve()
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
    setMode('follow')
    const pl = RUNDOWN.playlists[0]
    if (pl) {
      pl.items = [
        item('i1', 'Welcome', 'Verse 1'),
        item('i2', 'Pastor Dan', 'Chorus 1'),
        item('i3', 'Third', 'Chorus 1'),
      ]
    }
    return serve()
  }

  test('each section appears once, in running order', async () => {
    withSections()
    await paint()
    const map = screen.getByRole('group', { name: 'Sections' })
    const names = within(map).getAllByRole('button').map((b) => b.textContent)
    // Chorus 1 holds two slides and is ONE target. A map with a button per
    // slide is the list it was meant to replace.
    expect(names.length).toBe(2)
    expect(names[0]).toContain('Verse 1')
    expect(names[1]).toContain('Chorus 1')
  })

  test('clicking one jumps the cursor by NAME, and in follow mode takes', async () => {
    const calls = withSections()
    await paint()
    const map = screen.getByRole('group', { name: 'Sections' })
    await act(async () => {
      within(map).getByText('Chorus 1').click()
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

/**
 * A chorus that comes round three times is one Item in three places (ADR 0026).
 * Everything that marks a place — the cursor, the running numbers, the slide
 * grid — has to tell them apart, or the second chorus lights up the first.
 */
test('a repeated item is three places, and the cursor marks only the one it is on', async () => {
  const chorus = item('i1', 'Chorus', 'Chorus', 'e-1')
  // Labelled to suit the shared paint() helper, which waits for this row.
  const verse = item('i2', 'Pastor Dan', 'Verse 2', 'e-2')
  const again = { ...item('i1', 'Chorus', 'Chorus', 'e-3') }
  const before = RUNDOWN.playlists[0]!.items
  const cursorBefore = { ...RUNDOWN.cursor }
  RUNDOWN.playlists[0]!.items = [chorus, verse, again]
  RUNDOWN.cursor = {
    ...RUNDOWN.cursor,
    item: 'i1',
    entry: 'e-3',
    index: 2,
    total: 3,
    label: 'Chorus',
    previous: 'i2',
    previous_entry: 'e-2',
    next: null,
    next_entry: null,
  }
  try {
    serve()
    await paint()
    await screen.findAllByText('Chorus')

    // Three rows, numbered 01 02 03 — a Map keyed by item id would have
    // collapsed the two choruses into one number, and React would have warned
    // about duplicate keys besides.
    const rows = [...document.querySelectorAll('.c-rundownitem')]
    expect(rows.length).toBe(3)
    expect(rows.map((r) => r.querySelector('.c-rundownitem__id')?.textContent)).toEqual([
      '01', '02', '03',
    ])

    // Exactly one row is armed, and it is the LAST — matching on the item id
    // would have armed the first chorus instead.
    const armedIndex = rows.findIndex((r) => r.className.includes('armed'))
    expect(rows.filter((r) => r.className.includes('armed')).length).toBe(1)
    expect(armedIndex).toBe(2)
  } finally {
    RUNDOWN.playlists[0]!.items = before
    RUNDOWN.cursor = cursorBefore
  }
})
