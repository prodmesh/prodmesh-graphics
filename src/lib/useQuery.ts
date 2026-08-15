/**
 * The pull-side data layer, with a shared cache keyed by URL.
 *
 * Companion to useTopic (push). Two widgets asking for the same fact share one
 * request. Deliberately small — the same posture as ProdMesh-Control, which has
 * no data-fetching library either.
 */

import { useEffect, useState } from 'react'

type Entry<T> = { data?: T; error?: Error; promise?: Promise<void>; at: number }

const cache = new Map<string, Entry<unknown>>()
const STALE_MS = 5_000

export function useQuery<T>(key: string | null): { data?: T; error?: Error; loading: boolean } {
  const [, bump] = useState(0)

  useEffect(() => {
    if (!key) return
    let cancelled = false
    const wake = () => { if (!cancelled) bump((n) => n + 1) }

    const entry = cache.get(key) as Entry<T> | undefined
    if (entry && Date.now() - entry.at < STALE_MS) {
      // Fresh entry. If it already settled, render reads it below and there is
      // nothing to wait for. If it is still in flight, we MUST attach to it.
      //
      // Returning early here instead was a real bug: React runs an effect
      // twice on mount in development, so the second run found the first run's
      // in-flight entry, returned, and never re-rendered — while the first
      // run's own wake-up had already been cancelled by its cleanup. The widget
      // sat on "connecting" forever with the data sitting in the cache beside
      // it. The same thing happens in production whenever a component remounts
      // inside the stale window.
      if (!entry.data && !entry.error) entry.promise?.finally(wake)
      return () => { cancelled = true }
    }

    const next: Entry<T> = { at: Date.now() }
    next.promise = fetch(key)
      .then(async (r) => {
        if (!r.ok) throw new Error(`${r.status} ${r.statusText}`)
        next.data = (await r.json()) as T
      })
      .catch((e: Error) => { next.error = e })
      .finally(wake)
    cache.set(key, next as Entry<unknown>)

    return () => { cancelled = true }
  }, [key])

  const entry = key ? (cache.get(key) as Entry<T> | undefined) : undefined
  return { data: entry?.data, error: entry?.error, loading: !entry?.data && !entry?.error }
}

/** Drop a cache entry so the next read refetches. */
export function invalidate(key: string) {
  cache.delete(key)
}

/** Test seam. */
export function _reset() {
  cache.clear()
}
