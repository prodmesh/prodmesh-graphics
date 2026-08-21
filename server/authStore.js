/**
 * Stations, users, groups, permissions, sessions, audit. ADR 0013.
 *
 * Inherited from ProdMesh-Control wholesale, with the one deliberate change:
 * **a station may be trusted to operate with no user logged in**, scoped to
 * named outputs or layers. Authority that evaporates at the worst moment is not
 * a security control, it is an outage.
 *
 * # The posture of an unconfigured server, stated once
 *
 * With **no users configured, everything is permitted** — and `/api/health`
 * says so, loudly, so it is visible rather than a silent hole. A booth
 * appliance that ships locked and needs a password nobody wrote down is an
 * outage on Sunday; one that ships open and never mentions it is a different
 * kind of failure. Creating the first user turns enforcement on for good.
 *
 * # What is hashed and what is not
 *
 * PINs are scrypt-hashed with a per-user salt. Session tokens are random, and
 * only their SHA-256 is stored — a stolen database does not yield live
 * sessions. Station tokens are the same. Nothing here stores a secret it could
 * hand back.
 */

import { createHash, randomBytes, scryptSync, timingSafeEqual } from 'node:crypto'
import { handle } from './db.js'
import { newId } from './ids.js'

export const PERMISSIONS = [
  'graphics.operate',
  'graphics.rundown.edit',
  'graphics.templates.edit',
  'graphics.nodes.configure',
  'graphics.assets.manage',
  'graphics.settings.manage',
  'graphics.users.manage',
  'graphics.stations.manage',
  'graphics.logs.view',
]

const SESSION_TTL_MS = 8 * 60 * 60 * 1000
const SCRYPT_KEYLEN = 64
/** Login attempts allowed per station+username before a cool-off. */
const THROTTLE_MAX = 5
const THROTTLE_WINDOW_MS = 60_000

const attempts = new Map() // `${station}:${username}` -> { n, first }

const sha256 = (s) => createHash('sha256').update(s).digest('hex')

function hashPin(pin, salt = randomBytes(16).toString('hex')) {
  return { salt, hash: scryptSync(String(pin), salt, SCRYPT_KEYLEN).toString('hex') }
}

function pinMatches(pin, salt, expected) {
  const actual = scryptSync(String(pin), salt, SCRYPT_KEYLEN)
  const want = Buffer.from(expected, 'hex')
  // Constant time, so a wrong PIN cannot be narrowed down by how long the
  // comparison took.
  return actual.length === want.length && timingSafeEqual(actual, want)
}

// ---------------------------------------------------------------------------

/** True while nobody has been configured. See the posture note above. */
export function unconfigured() {
  return handle().prepare('SELECT COUNT(*) AS n FROM users').get().n === 0
}

export function createGroup(name, permissions, id = newId()) {
  const db = handle()
  db.transaction(() => {
    db.prepare('INSERT INTO groups (id, name, created_at) VALUES (?, ?, ?)').run(id, name, Date.now())
    const ins = db.prepare('INSERT INTO group_permissions (group_id, permission) VALUES (?, ?)')
    for (const p of permissions) ins.run(id, p)
  })()
  return { id, name, permissions }
}

/**
 * Create a user.
 *
 * **The first user gets the wildcard**, and that is a safety rule rather than a
 * convenience. Creating the first user turns enforcement on for good; a first
 * user with no group would turn it on and hold nothing — unable to create a
 * group, grant a permission, or add anybody else. The server would be locked
 * out of its own administration by the act of being secured, with no way back
 * that does not involve a text editor and the database file.
 *
 * So the account that turns permissions on is the account that can still
 * administer them. Every appliance's first run works this way, and the caller
 * is told it happened rather than left to discover it.
 */
export function createUser({ username, pin, groups = [], display_name = null }) {
  const db = handle()
  const { salt, hash } = hashPin(pin)
  const id = newId()
  const first = unconfigured()
  let grantedAdmin = false
  if (first && groups.length === 0) {
    const existing = db.prepare("SELECT id FROM groups WHERE name = 'Administrators'").get()
    groups = [existing?.id ?? createGroup('Administrators', ['*']).id]
    grantedAdmin = true
  }
  db.transaction(() => {
    db.prepare(
      `INSERT INTO users (id, username, display_name, pin_salt, pin_hash, created_at)
       VALUES (?, ?, ?, ?, ?, ?)`,
    ).run(id, username, display_name, salt, hash, Date.now())
    const ins = db.prepare('INSERT INTO user_groups (user_id, group_id) VALUES (?, ?)')
    for (const g of groups) ins.run(id, g)
  })()
  return { id, username, display_name, groups, granted_admin: grantedAdmin }
}

/** Every permission a user holds, as the union of their groups. */
export function permissionsOf(userId) {
  const rows = handle()
    .prepare(
      `SELECT DISTINCT gp.permission FROM user_groups ug
         JOIN group_permissions gp ON gp.group_id = ug.group_id
        WHERE ug.user_id = ?`,
    )
    .all(userId)
  const set = new Set(rows.map((r) => r.permission))
  // The wildcard is what makes the Administrators group administrative.
  return set.has('*') ? new Set(PERMISSIONS) : set
}

// ---------------------------------------------------------------------------
// Stations
// ---------------------------------------------------------------------------

/**
 * Register a station and return its token **once**.
 *
 * Registration grants no permissions. A station is an identity for the audit
 * log and, if someone later marks it trusted, a scope — never an authority in
 * itself.
 */
export function registerStation(name) {
  const token = randomBytes(32).toString('hex')
  const id = newId()
  handle()
    .prepare(
      `INSERT INTO stations (id, name, token_hash, trusted, scope, created_at)
       VALUES (?, ?, ?, 0, NULL, ?)`,
    )
    .run(id, name, sha256(token), Date.now())
  return { id, name, token }
}

export function stationByToken(token) {
  if (!token) return null
  return handle().prepare('SELECT * FROM stations WHERE token_hash = ?').get(sha256(token)) ?? null
}

/**
 * Mark a station trusted to operate with no user logged in.
 *
 * Scoped to named outputs or layers, never global, and it grants **only**
 * `graphics.operate`. The blast radius is meant to be listable in one place.
 */
export function setStationTrust(id, { trusted, scope = null }) {
  handle()
    .prepare('UPDATE stations SET trusted = ?, scope = ? WHERE id = ?')
    .run(trusted ? 1 : 0, scope ? JSON.stringify(scope) : null, id)
  return handle().prepare('SELECT id, name, trusted, scope FROM stations WHERE id = ?').get(id)
}

/**
 * Rename a station.
 *
 * A browser names itself when it registers, because a station has to exist
 * before anyone can mark it trusted — including an unattended display nobody
 * ever signs in on. "Chrome · macOS" is a placeholder for a person to replace
 * with the place: "Booth Mac", "Foyer wall".
 */
export function renameStation(id, name) {
  const info = handle().prepare('UPDATE stations SET name = ? WHERE id = ?').run(name, id)
  if (info.changes === 0) return null
  return handle().prepare('SELECT id, name, trusted, scope FROM stations WHERE id = ?').get(id)
}

/** Does a trusted station's scope cover this output and layer? */
export function trustCovers(station, { output, layer }) {
  if (!station?.trusted) return false
  if (!station.scope) return true // trusted with no scope is trusted everywhere
  const scope = JSON.parse(station.scope)
  const outOk = !scope.outputs?.length || scope.outputs.includes(output)
  const layerOk = !scope.layers?.length || scope.layers.includes(layer)
  return outOk && layerOk
}

// ---------------------------------------------------------------------------
// Sessions
// ---------------------------------------------------------------------------

export function login({ username, pin, stationId = null }) {
  const key = `${stationId ?? '-'}:${username}`
  const now = Date.now()
  const a = attempts.get(key)
  if (a && now - a.first < THROTTLE_WINDOW_MS && a.n >= THROTTLE_MAX) {
    // Per station AND username, so one jammed keypad cannot lock out the whole
    // building and a scripted attack cannot walk the user list from one place.
    return { error: 'throttled', retry_after_ms: THROTTLE_WINDOW_MS - (now - a.first) }
  }

  const db = handle()
  const user = db.prepare('SELECT * FROM users WHERE username = ?').get(username)
  const ok = user && pinMatches(pin, user.pin_salt, user.pin_hash)
  if (!ok) {
    attempts.set(key, a && now - a.first < THROTTLE_WINDOW_MS ? { n: a.n + 1, first: a.first } : { n: 1, first: now })
    audit({ user: null, station: stationId, action: 'auth.login', result: 'refused', detail: username })
    return { error: 'bad_credentials' }
  }

  attempts.delete(key)
  const token = randomBytes(32).toString('hex')
  db.prepare(
    'INSERT INTO sessions (token_hash, user_id, station_id, created_at, expires_at) VALUES (?, ?, ?, ?, ?)',
  ).run(sha256(token), user.id, stationId, now, now + SESSION_TTL_MS)
  audit({ user: user.id, station: stationId, action: 'auth.login', result: 'ok' })
  return { token, user: { id: user.id, username: user.username, display_name: user.display_name } }
}

/**
 * Set a user's PIN.
 *
 * Every session that user holds is dropped, because a PIN is changed either to
 * rotate it or because somebody should not be signed in any more, and leaving
 * eight hours of live sessions behind serves neither.
 *
 * There is no HTTP route to this. It exists for `npm run reset-pin`, run on the
 * machine holding the database — the recovery path for a first user whose PIN
 * is forgotten, which is otherwise a server locked out of its own
 * administration for good. Physical access to the server is the authority, the
 * same reasoning ADR 0013 uses for trusted stations in a booth.
 */
export function setPin(username, pin) {
  const db = handle()
  const user = db.prepare('SELECT id FROM users WHERE username = ?').get(username)
  if (!user) return null
  const { salt, hash } = hashPin(pin)
  db.transaction(() => {
    db.prepare('UPDATE users SET pin_salt = ?, pin_hash = ? WHERE id = ?').run(salt, hash, user.id)
    db.prepare('DELETE FROM sessions WHERE user_id = ?').run(user.id)
  })()
  audit({ user: user.id, station: null, action: 'auth.set_pin', result: 'ok', detail: 'console' })
  return { id: user.id, username }
}

export function logout(token) {
  handle().prepare('DELETE FROM sessions WHERE token_hash = ?').run(sha256(token ?? ''))
}

export function sessionOf(token) {
  if (!token) return null
  const row = handle()
    .prepare(
      `SELECT s.*, u.username, u.display_name FROM sessions s
         JOIN users u ON u.id = s.user_id
        WHERE s.token_hash = ?`,
    )
    .get(sha256(token))
  if (!row) return null
  if (row.expires_at < Date.now()) {
    handle().prepare('DELETE FROM sessions WHERE token_hash = ?').run(row.token_hash)
    return null
  }
  return row
}

/** Drop expired sessions. Cheap, and keeps a long-lived booth tidy. */
export function sweep() {
  handle().prepare('DELETE FROM sessions WHERE expires_at < ?').run(Date.now())
}

// ---------------------------------------------------------------------------
// Audit
// ---------------------------------------------------------------------------

/**
 * Record what happened and who did it.
 *
 * **A refusal is audited too.** A log that only records successes cannot answer
 * "why couldn't they take it on air?", which is the question actually asked
 * afterwards.
 */
export function audit({ user = null, station = null, action, result, detail = null }) {
  handle()
    .prepare(
      'INSERT INTO audit_log (at_utc, user_id, station_id, action, result, detail) VALUES (?, ?, ?, ?, ?, ?)',
    )
    .run(Date.now(), user, station, action, result, detail)
}

export function recentAudit(limit = 200) {
  return handle()
    .prepare(
      `SELECT a.*, u.username FROM audit_log a
         LEFT JOIN users u ON u.id = a.user_id
        ORDER BY a.at_utc DESC LIMIT ?`,
    )
    .all(Math.min(limit, 1000))
}

/** Test seam. */
export function _reset() {
  attempts.clear()
}
