/**
 * Health registry: declare / report / snapshot.
 *
 * Same shape as ProdMesh-Control's server/health.js, so the integration
 * conventions carry over. `healthKey(cfg)` is a stable key per integration and
 * every subsystem declares itself at boot rather than appearing on first error.
 */

const entries = new Map() // key -> { key, label, state, detail, since }

export function declare(key, label) {
  if (!entries.has(key)) {
    entries.set(key, { key, label, state: 'unknown', detail: null, since: Date.now() })
  }
}

export function report(key, state, detail = null) {
  const e = entries.get(key)
  if (!e) return
  if (e.state !== state) e.since = Date.now()
  e.state = state
  e.detail = detail
}

export function snapshot() {
  return [...entries.values()]
}

/**
 * Privacy-filtered view for unauthenticated displays.
 *
 * Built from a fixed field list rather than by deleting keys, so a new field
 * added upstream cannot leak a host, port, version, or error string onto a
 * screen on a wall. Same discipline as ProdMesh-Control's publicHealth().
 */
export function publicHealth() {
  return snapshot().map(({ key, label, state }) => ({ key, label, state }))
}
