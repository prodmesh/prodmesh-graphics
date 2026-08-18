/**
 * Show, tested for the promises it makes rather than for its markup.
 *
 * The claims worth defending: the take button does not lie, an item the node
 * has not staged is not armable, and what is on air comes from the node rather
 * than from optimism.
 */

import { afterEach, beforeEach, describe, expect, test, vi } from 'vitest'
import { act, render, screen, waitFor } from '@testing-library/react'
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

const ITEM = {
  id: 'item-1',
  take_id: null,
  label: 'Pastor Dan',
  template_id: 't1',
  template_rev: 'blake3:aa',
  data: {},
  destination: { layer: 'Lower Third' },
  destinations: [] as { node_id: string | null; output: string; layer: string;
                        template_rev: string; mode: string; blank: boolean }[],
  updated_at: 0,
}

const dest = (layer: string, mode: string) => ({
  node_id: null, output: 'out-0', layer, template_rev: 'blake3:aa', mode, blank: false,
})

/** Topic values the fake EventSource will deliver on subscribe. */
let topicValues: Record<string, unknown> = {}
let cueResponse: { status: number; body: unknown } = {
  status: 200,
  body: { result: 'applied', fired_at_frame: 1841944, at_utc: Date.now(), seq: 1 },
}

class FakeEventSource {
  static instances: FakeEventSource[] = []
  listeners = new Map<string, ((e: MessageEvent) => void)[]>()
  constructor(readonly url: string) {
    FakeEventSource.instances.push(this)
    // Deliver on a microtask, the way a real stream delivers its snapshot.
    queueMicrotask(() => {
      const topics = new URLSearchParams(url.split('?')[1]).get('topics')?.split(',') ?? []
      for (const t of topics) {
        if (t in topicValues) {
          this.emit('msg', { topic: t, data: topicValues[t] })
        }
      }
    })
  }
  addEventListener(type: string, fn: (e: MessageEvent) => void) {
    this.listeners.set(type, [...(this.listeners.get(type) ?? []), fn])
  }
  emit(type: string, payload: unknown) {
    for (const fn of this.listeners.get(type) ?? []) {
      fn({ data: JSON.stringify(payload) } as MessageEvent)
    }
  }
  close() {}
}

beforeEach(() => {
  resetQuery()
  resetStream()
  FakeEventSource.instances = []
  topicValues = {}
  ITEM.destinations = []
  vi.stubGlobal('EventSource', FakeEventSource)
  vi.stubGlobal('crypto', { ...globalThis.crypto, randomUUID: () => 'cue-fixed' })
  vi.stubGlobal('fetch', vi.fn(async (url: string, init?: RequestInit) => {
    if (String(url).startsWith('/api/cue')) {
      return new Response(JSON.stringify(cueResponse.body), {
        status: cueResponse.status,
        headers: { 'content-type': 'application/json' },
      })
    }
    const body =
      String(url) === '/api/nodes' ? { nodes: [NODE] }
      : String(url) === '/api/items' ? { items: [ITEM] }
      : {}
    void init
    return new Response(JSON.stringify(body), { status: 200 })
  }))
})

afterEach(() => vi.unstubAllGlobals())

/** A promise the test resolves by hand. (Node 20 has no Promise.withResolvers.) */
function deferred<T>() {
  let resolve!: (v: T) => void
  const promise = new Promise<T>((r) => { resolve = r })
  return { promise, resolve }
}

const paint = async () => {
  render(<Show />)
  await screen.findByText('Pastor Dan')
  // Let the stream deliver its first snapshot before asserting.
  await act(async () => { await Promise.resolve() })
}

describe('the take button', () => {
  test('waits for the node and does not go green on its own', async () => {
    await paint()
    const take = screen.getByRole('button', { name: /^Take$/ })

    // Hold the cue open so the in-flight state is observable at all.
    const held = deferred<Response>()
    vi.mocked(fetch).mockImplementationOnce(() => held.promise)

    act(() => { take.click() })
    // While the node has not answered, the button says so and stays disabled.
    // An optimistic update here would be the exact lie the synchronous endpoint
    // exists to prevent.
    await waitFor(() => expect(screen.getByRole('button', { name: /Taking/ })).toBeDefined())
    expect(screen.getByRole('button', { name: /Taking/ }).hasAttribute('disabled')).toBe(true)

    await act(async () => {
      held.resolve(new Response(JSON.stringify({ result: 'applied' }), { status: 200 }))
      await Promise.resolve()
    })
  })

  test('renders a 503 as the words the server sent', async () => {
    await paint()
    cueResponse = {
      status: 503,
      body: { error: 'cue_not_acked', label: 'Booth-1 didn’t confirm the take. Check the layer before taking again.' },
    }

    await act(async () => {
      screen.getByRole('button', { name: /^Take$/ }).click()
    })

    const alert = await screen.findByRole('alert')
    // Verbatim. The server's sentence already names the node and says what to
    // do; rewriting it in the browser would lose both.
    expect(alert.textContent).toContain('didn’t confirm the take')
    cueResponse = { status: 200, body: { result: 'applied' } }
  })

  test('reuses one cue_id for the press, so a double-click is one take', async () => {
    await paint()
    await act(async () => { screen.getByRole('button', { name: /^Take$/ }).click() })

    const call = vi.mocked(fetch).mock.calls.find((c) => String(c[0]) === '/api/cue')
    expect(call).toBeDefined()
    const body = JSON.parse(String((call![1] as RequestInit).body))
    expect(body.cue_id).toBe('cue-fixed')
    expect(body.surface).toBe('show_view')
    expect(body.op).toBe('take')
  })
})

describe('readiness', () => {
  test('an item the node has not staged cannot be taken', async () => {
    topicValues['item:item-1:readiness'] = { item: 'item-1', ready: false, missing: ['blake3:aa'] }
    await paint()

    await waitFor(() => {
      // Not a warning — not armable (ADR 0006). And it names what is missing
      // rather than just refusing.
      expect(screen.getByText(/still copying/)).toBeDefined()
      expect(screen.getByRole('button', { name: /^Take$/ }).hasAttribute('disabled')).toBe(true)
    })
  })

  test('an item ready on one destination and not the other is takeable, and says so', async () => {
    topicValues['item:item-1:readiness'] = {
      item: 'item-1', ready: true, partial: true, missing: ['blake3:bb'], unreadable: null,
      destinations: [
        { node: 'booth-1', output: 'out-0', layer: 'Lower Third', ready: true, missing: [], unreadable: null },
        { node: 'booth-2', output: 'out-0', layer: 'Wall', ready: false, missing: ['blake3:bb'], unreadable: null },
      ],
    }
    await paint()

    await waitFor(() => {
      // Takeable — ADR 0018 relaxes ADR 0006 to "at least one destination". The
      // old rule would grey this out, which means one un-staged asset on the
      // overflow feed stops the programme feed getting its lower third.
      expect(screen.getByRole('button', { name: /^Take$/ }).hasAttribute('disabled')).toBe(false)
      // And it must say which one will not fire, BEFORE the press. Afterwards
      // the aggregated ack reports it, which is too late to be a choice.
      expect(screen.getByText(/will not reach booth-2 out-0\/Wall/)).toBeDefined()
    })
  })
})

describe('what is on air', () => {
  test('comes from the node, in the present tense', async () => {
    topicValues['output:out-0:layers'] = {
      output: 'out-0', node: 'booth-1', fps: 59.94, late_frames: 0,
      layers: [
        { name: 'Lower Third', occupant: { item_id: 'item-1', item_label: 'Pastor Dan', template_rev: 'blake3:aa', phase: 'hold', timeline: null, t_ms: 0, degraded: false } },
        { name: 'Bug', occupant: null },
      ],
    }
    await paint()

    await waitFor(() => {
      const onair = screen.getByLabelText('On air')
      expect(onair.textContent).toContain('Lower Third')
      expect(onair.textContent).toContain('Pastor Dan')
      expect(onair.textContent).toContain('clear')
    })
  })

  test('a degraded graphic says so', async () => {
    topicValues['output:out-0:layers'] = {
      output: 'out-0', node: 'booth-1', fps: 59.94, late_frames: 0,
      layers: [{ name: 'L3', occupant: { item_id: 'item-1', item_label: 'X', template_rev: 'r', phase: 'hold', timeline: null, t_ms: 0, degraded: true } }],
    }
    await paint()
    // It is on air AND it is wrong. Saying nothing would be the failure.
    await waitFor(() => expect(screen.getByText(/degraded/)).toBeDefined())
  })
})

describe('a destination that waits to be asked for — ADR 0018', () => {
  test('no button when the item has nothing on request', async () => {
    ITEM.destinations = [dest('Lower Third', 'auto')]
    await paint()
    // Every row growing a dead button would be worse than the feature is good.
    expect(screen.queryByRole('button', { name: /Also full screen/ })).toBeNull()
  })

  test('the button appears, and fires take_also rather than take', async () => {
    ITEM.destinations = [dest('Lower Third', 'auto'), dest('Fullscreen', 'on_request')]
    await paint()

    const btn = await screen.findByRole('button', { name: /Also full screen/ })
    await act(async () => { btn.click(); await Promise.resolve() })

    const call = (globalThis.fetch as unknown as { mock: { calls: [string, RequestInit][] } })
      .mock.calls.find((c) => String(c[0]) === '/api/cue')
    expect(call).toBeDefined()
    const body = JSON.parse(String(call![1].body))
    // A separate op, not a flag: on a panel these are different buttons, and
    // in the log "the full screen went up" is a different sentence.
    expect(body.op).toBe('take_also')
    expect(body.item).toBe('item-1')
  })
})
