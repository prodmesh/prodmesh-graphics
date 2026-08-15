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

    const entry = cache.get(key) as Entry<T> | undefined
    if (entry && Date.now() - entry.at < STALE_MS) return

    const next: Entry<T> = { at: Date.now() }
    next.promise = fetch(key)
      .then(async (r) => {
        if (!r.ok) throw new Error(`${r.status} ${r.statusText}`)
        next.data = (await r.json()) as T
      })
      .catch((e: Error) => { next.error = e })
      .finally(() => { if (!cancelled) bump((n) => n + 1) })
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
