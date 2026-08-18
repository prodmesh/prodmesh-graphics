/**
 * useQuery, tested for the case that actually broke.
 */

import { afterEach, beforeEach, describe, expect, test, vi } from 'vitest'
import { render, screen, waitFor } from '@testing-library/react'
import { StrictMode } from 'react'
import { useQuery, invalidate, _reset } from './useQuery'

function Version() {
  const { data, loading } = useQuery<{ server: string }>('/api/version')
  return <p>{loading ? 'connecting' : `v${data?.server}`}</p>
}

beforeEach(() => {
  _reset()
  vi.stubGlobal('fetch', vi.fn(async () => {
    // A real network delay, so the second mount lands while the first fetch is
    // still in flight — which is the whole point.
    await new Promise((r) => setTimeout(r, 20))
    return new Response(JSON.stringify({ server: '0.1.0' }), { status: 200 })
  }))
})
afterEach(() => vi.unstubAllGlobals())

describe('a component that mounts twice', () => {
  test('still gets its data', async () => {
    // React runs effects twice on mount in StrictMode. The second run finds the
    // first run's in-flight cache entry; if it returns early without attaching
    // to it, the first run's wake-up has already been cancelled and nothing
    // ever re-renders. The widget sat on "connecting" forever with the data in
    // the cache beside it.
    render(
      <StrictMode>
        <Version />
      </StrictMode>,
    )
    await waitFor(() => expect(screen.getByText('v0.1.0')).toBeDefined())
  })

  test('two widgets asking for the same fact share one request', async () => {
    render(
      <>
        <Version />
        <Version />
      </>,
    )
    await waitFor(() => expect(screen.getAllByText('v0.1.0')).toHaveLength(2))
    expect(vi.mocked(fetch)).toHaveBeenCalledTimes(1)
  })
})

describe('invalidate', () => {
  test('refetches for a component that is already mounted', async () => {
    let version = 1
    vi.stubGlobal('fetch', vi.fn(async () => {
      await new Promise((r) => setTimeout(r, 5))
      return new Response(JSON.stringify({ server: `0.1.${version}` }), { status: 200 })
    }))

    render(<Version />)
    await waitFor(() => expect(screen.getByText('v0.1.1')).toBeDefined())

    // Deleting the cache entry alone does nothing here: the component's effect
    // depends on the key, the key has not changed, so it would never look
    // again. Every save-then-invalidate in the app silently failed to refresh
    // until invalidation became an event rather than a deletion.
    version = 2
    invalidate('/api/version')
    await waitFor(() => expect(screen.getByText('v0.1.2')).toBeDefined())
  })

  test('invalidating something nobody is watching does not fetch', () => {
    invalidate('/api/nothing-mounted')
    expect(vi.mocked(fetch)).not.toHaveBeenCalled()
  })

  test('keeps the last-known data on screen while the refetch is in flight', async () => {
    let version = 1
    let resolveRefetch: (() => void) | null = null
    vi.stubGlobal('fetch', vi.fn(async () => {
      if (version > 1) await new Promise<void>((r) => { resolveRefetch = r })
      return new Response(JSON.stringify({ server: `0.1.${version}` }), { status: 200 })
    }))

    render(<Version />)
    await waitFor(() => expect(screen.getByText('v0.1.1')).toBeDefined())

    // Between the write and the response the old value is the last known good
    // state. Passing through `undefined` here made every consumer that renders
    // nothing without data unmount for a frame — Show's whole console among
    // them, which tore down the preview socket on every take.
    version = 2
    invalidate('/api/version')
    expect(screen.getByText('v0.1.1')).toBeDefined()
    expect(screen.queryByText('connecting')).toBeNull()

    resolveRefetch!()
    await waitFor(() => expect(screen.getByText('v0.1.2')).toBeDefined())
  })
})
