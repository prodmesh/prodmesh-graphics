/**
 * The push-side data layer: one EventSource per tab, multiplexed by topic.
 *
 * Companion to useQuery (pull). Ported from ProdMesh-Control's src/lib/stream.ts
 * and for the same reason: a LAN appliance with no TLS gets no HTTP/2, so the
 * browser's six-connections-per-origin limit is real. Twelve widgets mounting
 * together must open one connection, not twelve.
 *
 * ---------------------------------------------------------------------------
 * Topics carry SNAPSHOTS. The server conflates under backpressure — newest wins
 * — which is correct only because a client renders the newest value and nothing
 * else.
 *
 * If you want to observe *events* (a take fired), do not look for a topic that
 * delivers them one at a time. There isn't one, on purpose. Subscribe to
 * `show:<id>:cuelog`, which publishes { head_seq, entries[] } — the snapshot of
 * a log tail — and fetch any gap with GET /api/cuelog?since=<seq>.
 *
 * ADR 0008.
 * ---------------------------------------------------------------------------
 */

import { useSyncExternalStore } from 'react'

type Listener = () => void

const values = new Map<string, unknown>()
const listeners = new Map<string, Set<Listener>>()
const refcount = new Map<string, number>()

let source: EventSource | null = null
let reconnectTimer: ReturnType<typeof setTimeout> | null = null

/** Debounced so twelve widgets mounting in one tick open one connection. */
const RECONNECT_DEBOUNCE_MS = 30

function scheduleReconnect() {
  if (reconnectTimer !== null) clearTimeout(reconnectTimer)
  reconnectTimer = setTimeout(() => {
    reconnectTimer = null
    connect()
  }, RECONNECT_DEBOUNCE_MS)
}

function connect() {
  source?.close()
  source = null

  const topics = [...refcount.entries()].filter(([, n]) => n > 0).map(([t]) => t)
  if (topics.length === 0) return

  const es = new EventSource(`/api/stream?topics=${encodeURIComponent(topics.join(','))}`)
  source = es

  es.addEventListener('msg', (e) => {
    const { topic, data } = JSON.parse((e as MessageEvent).data)
    values.set(topic, data)
    listeners.get(topic)?.forEach((fn) => fn())
  })

  es.addEventListener('rejected', (e) => {
    // The server drops invalid topics individually rather than failing the
    // connection: one stale widget in a saved layout must not blank the rest.
    const { topics: bad } = JSON.parse((e as MessageEvent).data)
    console.warn('stream: topics rejected', bad)
  })

  // EventSource reconnects on its own; this catches the case where the topic
  // set changed while the socket was down.
  es.addEventListener('error', () => scheduleReconnect())
}

function subscribe(topic: string, fn: Listener): () => void {
  let set = listeners.get(topic)
  if (!set) { set = new Set(); listeners.set(topic, set) }
  set.add(fn)

  const before = refcount.get(topic) ?? 0
  refcount.set(topic, before + 1)
  if (before === 0) scheduleReconnect()

  return () => {
    set!.delete(fn)
    const n = (refcount.get(topic) ?? 1) - 1
    refcount.set(topic, n)
    // Refcounted all the way down: when the last browser stops watching a node,
    // the server stops polling it.
    if (n === 0) scheduleReconnect()
  }
}

/** Subscribe to a snapshot topic. Returns the newest value, or undefined. */
export function useTopic<T = unknown>(topic: string): T | undefined {
  return useSyncExternalStore(
    (fn) => subscribe(topic, fn),
    () => values.get(topic) as T | undefined,
    () => undefined,
  )
}

/** Test seam. */
export function _reset() {
  source?.close()
  source = null
  values.clear()
  listeners.clear()
  refcount.clear()
}
