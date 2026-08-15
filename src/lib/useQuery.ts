/**
 * The pull-side data layer, with a shared cache keyed by URL.
 *
 * Companion to useTopic (push). Two widgets asking for the same fact share one
 * request. Deliberately small — the same posture as ProdMesh-Control, which has
 * no data-fetching library either.
 *
 * # Why there is a subscriber registry and not just a cache
 *
 * The first version had only the cache, and `invalidate()` merely deleted the
 * entry. That does nothing for a component that is already mounted: its effect
 * depends on `[key]`, the key has not changed, so it never refetches. Every
 * save-then-invalidate in the app silently failed to refresh, and it *looked*
 * like it worked wherever the component happened to unmount in between —
 * closing an editor, switching modes. Where nothing unmounted, the screen kept
 * showing pre-save data.
 *
 * So invalidation is an event, not a deletion: subscribers are told, and they
 * refetch.
 */

import { useEffect, useState } from 'react'

type Entry<T> = { data?: T; error?: Error; promise?: Promise<void>; at: number }

const cache = new Map<string, Entry<unknown>>()
const subscribers = new Map<string, Set<() => void>>()
const STALE_MS = 5_000

function notify(key: string) {
  subscribers.get(key)?.forEach((fn) => fn())
}

function start<T>(key: string): Entry<T> {
  const next: Entry<T> = { at: Date.now() }
  next.promise = fetch(key)
    .then(async (r) => {
      if (!r.ok) throw new Error(`${r.status} ${r.statusText}`)
      next.data = (await r.json()) as T
    })
    .catch((e: Error) => {
      next.error = e
    })
    .finally(() => notify(key))
  cache.set(key, next as Entry<unknown>)
  return next
}

export function useQuery<T>(key: string | null): { data?: T; error?: Error; loading: boolean } {
  const [, bump] = useState(0)

  useEffect(() => {
    if (!key) return
    const wake = () => bump((n) => n + 1)

    let set = subscribers.get(key)
    if (!set) {
      set = new Set()
      subscribers.set(key, set)
    }
    set.add(wake)

    const entry = cache.get(key) as Entry<T> | undefined
    if (!entry || Date.now() - entry.at >= STALE_MS) {
      start<T>(key)
    } else if (!entry.data && !entry.error) {
      // Fresh but still in flight. React runs an effect twice on mount, so the
      // second run finds the first run's pending entry — it must wait on that
      // promise rather than return, or nothing ever re-renders and the widget
      // sits on its loading state with the data arriving into the cache beside
      // it. The same happens on any remount inside the stale window.
      entry.promise?.finally(wake)
    }

    return () => {
      set.delete(wake)
      if (set.size === 0) subscribers.delete(key)
    }
  }, [key])

  const entry = key ? (cache.get(key) as Entry<T> | undefined) : undefined
  return { data: entry?.data, error: entry?.error, loading: !entry?.data && !entry?.error }
}

/**
 * Drop a cache entry and refetch it for anyone watching.
 *
 * Deleting alone is not enough: a mounted component's effect depends on the
 * key, which has not changed, so it would never look again. Callers use this
 * after a write, and they are entitled to expect the screen to catch up.
 */
export function invalidate(key: string) {
  cache.delete(key)
  if (subscribers.has(key)) {
    start(key)
  }
  notify(key)
}

/** Test seam. */
export function _reset() {
  cache.clear()
  subscribers.clear()
}
