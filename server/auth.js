/**
 * The permission boundary. ADR 0013.
 *
 * # The server is the boundary; the UI only hides things
 *
 * Every mutating route goes through `requirePermission`. The browser hides
 * controls it would be refused because that is kinder, not because it is
 * enforcement — and nothing here trusts anything the browser says about who it
 * is beyond the token it presents.
 *
 * # 401 and 403 mean different things, deliberately
 *
 * **401: nobody is logged in. 403: this operator lacks the permission.** They
 * are the browser's only way to tell "sign in" from "ask someone else", and
 * collapsing them into one status makes both messages wrong.
 */

import * as store from './authStore.js'

/**
 * Resolve who is asking. Never throws; an unreadable token is simply nobody.
 */
export function identify(req) {
  const bearer = /^Bearer (.+)$/i.exec(req.get('authorization') ?? '')?.[1] ?? null
  const session = bearer ? store.sessionOf(bearer) : null
  const station = store.stationByToken(req.get('x-prodmesh-station'))
  return {
    token: bearer,
    user: session ? { id: session.user_id, username: session.username } : null,
    station: station ? { id: station.id, name: station.name, trusted: Boolean(station.trusted), scope: station.scope } : null,
    permissions: session ? store.permissionsOf(session.user_id) : new Set(),
  }
}

export function attach(req, _res, next) {
  req.who = identify(req)
  next()
}

const LABELS = {
  'graphics.operate': 'Take graphics on air',
  'graphics.rundown.edit': 'Edit items, playlists and the rundown',
  'graphics.templates.edit': 'Design and publish templates',
  'graphics.nodes.configure': 'Configure playout nodes',
  'graphics.assets.manage': 'Manage assets',
  'graphics.settings.manage': 'Change server settings',
  'graphics.users.manage': 'Manage users and groups',
  'graphics.stations.manage': 'Manage stations',
  'graphics.logs.view': 'View logs',
}

/**
 * Require a permission.
 *
 * `scopeOf(req)` is only consulted for `graphics.operate`, and only to decide
 * whether a **trusted station** covers this particular output and layer. Trust
 * is never global and never grants anything but operating.
 */
export function requirePermission(permission, scopeOf = null) {
  return (req, res, next) => {
    const who = req.who ?? identify(req)

    // An unconfigured server permits everything, and /api/health says so. A
    // booth appliance that ships locked behind a password nobody wrote down is
    // an outage; one that ships open and never mentions it is a quiet hole.
    // The first user turns this off for good.
    if (store.unconfigured()) return next()

    if (who.permissions.has(permission)) {
      store.audit({ user: who.user?.id, station: who.station?.id, action: permission, result: 'ok' })
      return next()
    }

    // The one deliberate weakening in ADR 0013: a station trusted for
    // operation, within its scope, with nobody logged in. Recorded as the
    // station acting with no user, which is the honest record.
    if (permission === 'graphics.operate' && who.station?.trusted) {
      const scope = scopeOf ? scopeOf(req) : {}
      if (store.trustCovers({ trusted: true, scope: who.station.scope }, scope)) {
        store.audit({ user: null, station: who.station.id, action: permission, result: 'ok_trusted_station' })
        return next()
      }
    }

    store.audit({
      user: who.user?.id, station: who.station?.id,
      action: permission, result: 'refused', detail: req.originalUrl,
    })
    const status = who.user ? 403 : 401
    res.status(status).json({
      error: 'permission_required',
      permission,
      // Name the authority that is missing, so the message is actionable
      // rather than a shrug.
      label: who.user
        ? `You don’t have permission to: ${LABELS[permission] ?? permission}.`
        : 'Sign in to do that.',
    })
  }
}
