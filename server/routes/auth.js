/**
 * Authentication and user administration. ADR 0013 · `docs/specs/http-api.md` §5.
 */

import express from 'express'
import * as store from '../authStore.js'
import { requirePermission } from '../auth.js'
import { handle } from '../db.js'

export const router = express.Router()

const fail = (res, status, error, label) => res.status(status).json({ error, label })

router.post('/api/stations/register', (req, res) => {
  const name = String(req.body?.name ?? '').trim()
  if (!name) return fail(res, 422, 'validation_failed', 'A station needs a name.')
  // The token is returned ONCE and only its hash is kept. There is no endpoint
  // that can hand it back, because there is nothing to hand back.
  res.status(201).json(store.registerStation(name))
})

router.post('/api/auth/login', (req, res) => {
  const { username, pin } = req.body ?? {}
  if (!username || !pin) return fail(res, 422, 'validation_failed', 'Enter a username and PIN.')
  const r = store.login({ username, pin, stationId: req.who?.station?.id ?? null })

  if (r.error === 'throttled') {
    return res.status(429).json({
      error: 'throttled',
      label: `Too many attempts. Try again in ${Math.ceil(r.retry_after_ms / 1000)} seconds.`,
    })
  }
  if (r.error) {
    // One message for "no such user" and "wrong PIN". Distinguishing them tells
    // an attacker which usernames exist.
    return fail(res, 401, 'bad_credentials', 'That username and PIN don’t match.')
  }
  res.json({ ...r, permissions: [...store.permissionsOf(r.user.id)] })
})

router.post('/api/auth/logout', (req, res) => {
  const bearer = /^Bearer (.+)$/i.exec(req.get('authorization') ?? '')?.[1]
  if (bearer) store.logout(bearer)
  res.status(204).end()
})

/**
 * Who am I, and what may I do.
 *
 * The browser uses this to hide controls it would be refused. That is guidance;
 * the server is the boundary.
 */
router.get('/api/auth/me', (req, res) => {
  res.json({
    user: req.who?.user ?? null,
    station: req.who?.station ?? null,
    permissions: [...(req.who?.permissions ?? [])],
    // Said out loud rather than inferred, so a UI cannot quietly present an
    // open server as a secured one.
    unconfigured: store.unconfigured(),
    // The opposite failure, and it needs saying just as loudly: enforcement is
    // on and NOBODY can administer this server. Every route refuses, including
    // the ones that would fix it, and being refused everywhere with no pattern
    // to it reads as the system being broken rather than as one missing row.
    no_administrator: !store.unconfigured() && !store.administrable(),
  })
})

// ---------------------------------------------------------------------------

router.get('/api/users', requirePermission('graphics.users.manage'), (_req, res) => {
  const db = handle()
  // A group WITHOUT its permissions is the one thing this list must not show:
  // it renders as holding nothing, and the group it lies about most loudly is
  // Administrators.
  const perms = db.prepare('SELECT group_id, permission FROM group_permissions').all()
  res.json({
    users: db.prepare('SELECT id, username, display_name, created_at FROM users ORDER BY username').all(),
    groups: db
      .prepare('SELECT * FROM groups ORDER BY name')
      .all()
      .map((g) => ({ ...g, permissions: perms.filter((p) => p.group_id === g.id).map((p) => p.permission) })),
  })
})

router.post('/api/users', requirePermission('graphics.users.manage'), (req, res) => {
  const { username, pin, groups = [], display_name = null } = req.body ?? {}
  if (!username || !pin) return fail(res, 422, 'validation_failed', 'A user needs a username and a PIN.')
  if (String(pin).length < 4) return fail(res, 422, 'validation_failed', 'A PIN needs at least four digits.')
  try {
    res.status(201).json(store.createUser({ username, pin, groups, display_name }))
  } catch (e) {
    if (String(e.message).includes('UNIQUE')) {
      return fail(res, 409, 'already_exists', 'There is already a user with that name.')
    }
    throw e
  }
})

router.post('/api/groups', requirePermission('graphics.users.manage'), (req, res) => {
  const { name, permissions = [] } = req.body ?? {}
  if (!name) return fail(res, 422, 'validation_failed', 'A group needs a name.')
  const unknown = permissions.filter((p) => p !== '*' && !store.PERMISSIONS.includes(p))
  if (unknown.length) {
    // A typo'd permission silently grants nothing, which looks like the system
    // being broken rather than the group being wrong.
    return fail(res, 422, 'validation_failed', `Not a permission: ${unknown.join(', ')}.`)
  }
  res.status(201).json(store.createGroup(name, permissions))
})

router.get('/api/stations', requirePermission('graphics.stations.manage'), (_req, res) => {
  res.json({
    stations: handle()
      .prepare('SELECT id, name, trusted, scope, created_at FROM stations ORDER BY name')
      .all()
      .map((s) => ({ ...s, trusted: Boolean(s.trusted), scope: s.scope ? JSON.parse(s.scope) : null })),
  })
})

/**
 * Rename a station.
 *
 * The browser registers itself with a placeholder — it has to, so an unattended
 * display can be marked trusted without anybody ever signing in there — and
 * this is where that placeholder becomes the name of a place.
 */
router.put('/api/stations/:id', requirePermission('graphics.stations.manage'), (req, res) => {
  const name = String(req.body?.name ?? '').trim()
  if (!name) return fail(res, 422, 'validation_failed', 'A station needs a name.')
  const s = store.renameStation(req.params.id, name)
  if (!s) return fail(res, 404, 'not_found', 'There is no station with that id.')
  res.json({ ...s, trusted: Boolean(s.trusted), scope: s.scope ? JSON.parse(s.scope) : null })
})

/**
 * Mark a station trusted to operate with no user logged in.
 *
 * Every trusted station is listable from one place on purpose: the blast radius
 * of this weakening has to be visible, not scattered through settings.
 */
router.put('/api/stations/:id/trust', requirePermission('graphics.stations.manage'), (req, res) => {
  const s = store.setStationTrust(req.params.id, {
    trusted: req.body?.trusted !== false,
    scope: req.body?.scope ?? null,
  })
  if (!s) return fail(res, 404, 'not_found', 'There is no station with that id.')
  store.audit({
    user: req.who?.user?.id, station: req.who?.station?.id,
    action: 'station.trust', result: 'ok', detail: `${req.params.id} trusted=${s.trusted}`,
  })
  res.json({ ...s, trusted: Boolean(s.trusted), scope: s.scope ? JSON.parse(s.scope) : null })
})

router.get('/api/audit', requirePermission('graphics.logs.view'), (req, res) => {
  res.json({ entries: store.recentAudit(Number(req.query.limit ?? 200)) })
})
