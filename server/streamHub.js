/**
 * SSE topic hub — SNAPSHOTS ONLY.
 *
 * Ported from ProdMesh-Control's server/streamHub.js, which solved this for a
 * dashboard. Same model: one EventSource per browser tab, multiplexed by named
 * topic, refcounted producers, and conflation under backpressure.
 *
 * SSE rather than WebSocket for the reason that app documented: a LAN appliance
 * with no TLS gets no HTTP/2, so the browser's six-connections-per-origin limit
 * is real and unfixable at this layer.
 *
 * ---------------------------------------------------------------------------
 * THE RULE THIS FILE EXISTS TO ENFORCE
 *
 * Under backpressure this hub keeps at most ONE pending frame per topic per
 * connection, newest wins. That is correct *because every topic carries a state
 * snapshot*: a client only renders the newest value, so superseded ones cost
 * nothing to drop.
 *
 * It is WRONG for an event-shaped topic. A superseded take is not a stale value
 * a newer one corrects — it is a graphic that never went on air, and nothing
 * downstream will ever notice.
 *
 * So: cues go over PMG Link, acked (ADR 0007). And when an event feed needs to
 * reach a browser, it does so as THE SNAPSHOT OF A LOG TAIL — see cueLog.js.
 * Conflation is safe again because the newest frame contains everything the
 * dropped frames carried.
 *
 * NEVER publish a bare {event:"take", ...} frame on a topic here. ADR 0008.
 * ---------------------------------------------------------------------------
 */

const MAX_TOPICS = 64
const HEARTBEAT_MS = 20_000

/** pattern -> { valid, start, stop, snapshot } */
const registry = new Map()
/** resolved topic -> { subscribers:Set<conn>, producer, retained } */
const topics = new Map()
const connections = new Set()

/**
 * Claim a topic pattern, e.g. 'node:*:health'.
 *
 * @param {string} pattern
 * @param {object} handlers
 * @param {(params: string[]) => boolean} handlers.valid
 *        Reject topics naming nothing real. Subscribing STARTS WORK, so a hub
 *        that accepts any string is a resource-exhaustion primitive.
 * @param {(params: string[], publish: (data: any) => void) => any} handlers.start
 *        First subscriber arrived; begin producing. Return a handle for stop().
 * @param {(handle: any) => void} handlers.stop
 *        Last subscriber left; stop producing. Nobody polls a node no browser
 *        is watching.
 * @param {(params: string[]) => any} [handlers.snapshot]
 *        Current value, so a widget mounting mid-service paints at once.
 */
export function registerTopic(pattern, handlers) {
  if (registry.has(pattern)) throw new Error(`topic pattern already registered: ${pattern}`)
  registry.set(pattern, handlers)
}

function match(topic) {
  const parts = topic.split(':')
  for (const [pattern, handlers] of registry) {
    const pp = pattern.split(':')
    if (pp.length !== parts.length) continue
    const params = []
    let ok = true
    for (let i = 0; i < pp.length; i++) {
      if (pp[i] === '*') params.push(parts[i])
      else if (pp[i] !== parts[i]) { ok = false; break }
    }
    if (ok) return { handlers, params }
  }
  return null
}

/**
 * Publish a snapshot. Conflates under backpressure — newest wins.
 *
 * If what you are publishing is an event rather than a state snapshot, this is
 * the wrong function and there is no right one here. See the header.
 */
export function publish(topic, data) {
  const entry = topics.get(topic)
  if (!entry) return
  entry.retained = data
  const frame = `event: msg\ndata: ${JSON.stringify({ topic, data })}\n\n`
  for (const conn of entry.subscribers) send(conn, topic, frame)
}

function send(conn, topic, frame) {
  if (conn.blocked) {
    // At most one pending frame per topic per connection. res.write() returns
    // false once the socket buffer fills and Node then buffers without bound,
    // so ignoring backpressure makes this a memory-exhaustion vector — and more
    // mundanely, a booth Mac that sleeps costs us memory for as long as it does.
    conn.pending.set(topic, frame)
    return
  }
  if (!conn.res.write(frame)) {
    conn.blocked = true
    conn.res.once('drain', () => {
      conn.blocked = false
      const pending = [...conn.pending.values()]
      conn.pending.clear()
      for (const f of pending) send(conn, topic, f)
    })
  }
}

function subscribe(conn, topic) {
  if (conn.topics.size >= MAX_TOPICS) return false
  if (conn.topics.has(topic)) return true

  let entry = topics.get(topic)
  if (!entry) {
    const m = match(topic)
    // One stale widget in a saved layout must not blank the other eleven, so an
    // invalid topic is dropped individually rather than failing the connection.
    if (!m || !m.handlers.valid(m.params)) return false
    entry = { subscribers: new Set(), producer: null, retained: undefined }
    topics.set(topic, entry)
    entry.producer = m.handlers.start(m.params, (data) => publish(topic, data))
    if (m.handlers.snapshot) entry.retained = m.handlers.snapshot(m.params)
  }

  entry.subscribers.add(conn)
  conn.topics.add(topic)
  if (entry.retained !== undefined) {
    send(conn, topic, `event: msg\ndata: ${JSON.stringify({ topic, data: entry.retained })}\n\n`)
  }
  return true
}

function unsubscribeAll(conn) {
  for (const topic of conn.topics) {
    const entry = topics.get(topic)
    if (!entry) continue
    entry.subscribers.delete(conn)
    if (entry.subscribers.size === 0) {
      const m = match(topic)
      m?.handlers.stop?.(entry.producer)
      topics.delete(topic)
    }
  }
  conn.topics.clear()
  connections.delete(conn)
}

/** GET /api/stream?topics=a,b,c */
export function handleStream(req, res) {
  res.writeHead(200, {
    'Content-Type': 'text/event-stream',
    'Cache-Control': 'no-cache',
    Connection: 'keep-alive',
    'X-Accel-Buffering': 'no',
  })
  res.write('\n')

  const conn = { res, topics: new Set(), pending: new Map(), blocked: false }
  connections.add(conn)

  const requested = String(req.query?.topics ?? '').split(',').map((t) => t.trim()).filter(Boolean)
  const rejected = requested.filter((t) => !subscribe(conn, t))
  if (rejected.length) {
    res.write(`event: rejected\ndata: ${JSON.stringify({ topics: rejected })}\n\n`)
  }

  const beat = setInterval(() => res.write(': ping\n\n'), HEARTBEAT_MS)
  req.on('close', () => { clearInterval(beat); unsubscribeAll(conn) })
}

/** Diagnostics for /api/health. */
export function stats() {
  return {
    connections: connections.size,
    activeTopics: topics.size,
    registeredPatterns: [...registry.keys()],
  }
}

/** Test seam — drops all state. */
export function _reset() {
  // Snapshot first: unsubscribeAll() deletes from `connections` as it goes.
  const open = Array.from(connections)
  for (const conn of open) unsubscribeAll(conn)
  registry.clear()
  topics.clear()
}
