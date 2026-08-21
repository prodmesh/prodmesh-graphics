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

import { authHeaders } from './identity'

type Entry<T> = { data?: T; error?: Error; promise?: Promise<void>; at: number }

const cache = new Map<string, Entry<unknown>>()
const subscribers = new Map<string, Set<() => void>>()
const STALE_MS = 5_000

function notify(key: string) {
  subscribers.get(key)?.forEach((fn) => fn())
}

function start<T>(key: string): Entry<T> {
  // Carry the last-known data forward while the refetch is in flight. A
  // refetch is not an unknown: blanking subscribers between the write and the
  // response made every consumer that renders nothing without data — the
  // whole Show console among them — unmount for a frame on every take, which
  // tore down the preview socket. Same principle as the wall display: show
  // the last known good state, never a loading hole.
  const prev = cache.get(key) as Entry<T> | undefined
  const next: Entry<T> = { at: Date.now(), data: prev?.data }
  next.promise = fetch(key, { headers: authHeaders() })
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
 * Refetch a key for anyone watching; forget it if nobody is.
 *
 * Deleting alone is not enough: a mounted component's effect depends on the
 * key, which has not changed, so it would never look again. Callers use this
 * after a write, and they are entitled to expect the screen to catch up.
 *
 * For a watched key this does NOT pass through an empty state — the stale
 * value stays on screen until the fresh one lands (see start()). Between a
 * write and its refetch the old value is the last known good state, and a
 * subscriber seeing `undefined` there treated it as "gone" rather than
 * "changing" — unmounting, in Show's case, the preview socket under the
 * operator on every take.
 */
export function invalidate(key: string) {
  if (subscribers.has(key)) {
    start(key)
  } else {
    cache.delete(key)
  }
  notify(key)
}

/**
 * Refetch everything anyone is watching, and forget the rest.
 *
 * Signing in or out changes what every read is allowed to return, so the whole
 * cache is answers to a question that was asked as somebody else.
 */
export function invalidateAll() {
  // A snapshot: invalidate() deletes unwatched keys as it goes.
  for (const key of Array.from(cache.keys())) invalidate(key)
}

/** Test seam. */
export function _reset() {
  cache.clear()
  subscribers.clear()
}
