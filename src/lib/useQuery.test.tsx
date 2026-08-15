/**
 * useQuery, tested for the case that actually broke.
 */

import { afterEach, beforeEach, describe, expect, test, vi } from 'vitest'
import { render, screen, waitFor } from '@testing-library/react'
import { StrictMode } from 'react'
import { useQuery, _reset } from './useQuery'

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
