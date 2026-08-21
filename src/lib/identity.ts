/**
 * Who is at this browser, and where "here" is. ADR 0013, ADR 0025.
 *
 * # Two identities, not one
 *
 * A **station** is the machine. It registers once, keeps a token in local
 * storage, and grants NOTHING — it says where an action happened, which is what
 * makes the audit log answer "who took that, and from where". A **user** signs
 * in at a station with a username and a PIN and gets an eight-hour bearer token.
 *
 * That split is why a wall display can be given no credentials and still show
 * what is on air, and why a booth machine can be marked trusted to operate with
 * nobody signed in at all.
 *
 * # Registration is automatic, and it has to be
 *
 * The station registers itself on first load rather than asking somebody to
 * name it. An unattended display must be able to become a *trusted* station
 * (ADR 0013), and it cannot be marked trusted if it does not exist until
 * somebody signs in on it. The provisional name is the browser and platform,
 * because a list of stations called "Unknown" helps nobody; Configure renames
 * it.
 *
 * # Unknown identity answers YES
 *
 * `can()` returns true while identity is unknown. The status request has not
 * landed yet, and a control that starts disabled and enables a moment later
 * reads as broken. Guessing wrong this way costs a clear sentence from the
 * server; guessing wrong the other way makes a permitted operator believe they
 * are locked out mid-service. **The server is the boundary. This is guidance.**
 */

import { createContext, useContext } from 'react'

const STATION_KEY = 'pmg.station'
const SESSION_KEY = 'pmg.session'

/** A browser with storage disabled still works; it just re-registers. */
function read(key: string): string | null {
  try {
    return window.localStorage.getItem(key)
  } catch {
    return null
  }
}

function write(key: string, value: string | null) {
  try {
    if (value === null) window.localStorage.removeItem(key)
    else window.localStorage.setItem(key, value)
  } catch {
    /* nothing to do — the session lives for this page instead */
  }
}

export type Me = {
  user: { id: string; username: string } | null
  station: { id: string; name: string; trusted: boolean; scope: unknown } | null
  permissions: string[]
  /** Said out loud by the server, never inferred: no users exist yet. */
  unconfigured: boolean
}

export const stationToken = () => read(STATION_KEY)
export const sessionToken = () => read(SESSION_KEY)

/** The headers every request carries. Both are optional and both are honest. */
export function authHeaders(): Record<string, string> {
  const session = sessionToken()
  const station = stationToken()
  return {
    ...(session ? { authorization: `Bearer ${session}` } : {}),
    ...(station ? { 'x-prodmesh-station': station } : {}),
  }
}

// ---------------------------------------------------------------------------
// Events. The shell listens; anything may dispatch.

/** Identity changed — signed in, signed out, or a session expired. */
export const AUTH_CHANGED = 'pmg:auth-changed'
/** Something was refused. The detail names the authority that was missing. */
export const AUTH_REQUIRED = 'pmg:auth-required'

export type Denial = { permission?: string; label?: string }

export function requestSignIn(detail: Denial = {}) {
  window.dispatchEvent(new CustomEvent(AUTH_REQUIRED, { detail }))
}

/**
 * Tell the shell to look at identity again.
 *
 * Signing in and out are the obvious callers. The one that is not obvious, and
 * cost a confusing minute the first time somebody secured a server: **adding
 * the first user turns permissions on**, and until the shell knows, the footer
 * goes on saying the server is open and the header goes on not offering a way
 * in — on the very page where it just happened.
 */
export const identityChanged = () => window.dispatchEvent(new Event(AUTH_CHANGED))

const changed = identityChanged

// ---------------------------------------------------------------------------

/** Chrome · macOS. Provisional, renameable, and better than "Unknown". */
function describeThisMachine(): string {
  const ua = navigator.userAgent
  const browser =
    /Edg\//.test(ua) ? 'Edge'
    : /OPR\//.test(ua) ? 'Opera'
    : /Chrome\//.test(ua) ? 'Chrome'
    : /Firefox\//.test(ua) ? 'Firefox'
    : /Safari\//.test(ua) ? 'Safari'
    : 'Browser'
  const platform =
    /Mac/.test(ua) ? 'macOS'
    : /Windows/.test(ua) ? 'Windows'
    : /Linux/.test(ua) ? 'Linux'
    : 'this machine'
  return `${browser} · ${platform}`
}

/**
 * Register this browser as a station, once, ever.
 *
 * The token comes back exactly once and only its hash is kept, so losing local
 * storage means becoming a new station — not recovering the old one. That is
 * why re-registration is silent rather than an error.
 */
export async function ensureStation(): Promise<void> {
  if (stationToken()) return
  try {
    const res = await fetch('/api/stations/register', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ name: describeThisMachine() }),
    })
    if (!res.ok) return
    const body = (await res.json()) as { token?: unknown }
    if (typeof body.token === 'string') write(STATION_KEY, body.token)
  } catch {
    // The server is unreachable. A station identity is not worth a retry loop
    // in front of a console that still has to render what is on air.
  }
}

export async function fetchMe(): Promise<Me | null> {
  try {
    const res = await fetch('/api/auth/me', { headers: authHeaders() })
    if (!res.ok) return null
    const me = (await res.json()) as Me
    // An eight-hour session outlives most services and expires during none of
    // them, but it does expire. Drop the dead token rather than sending it
    // forever and reading 401s as "you are not allowed".
    if (!me.user && sessionToken()) write(SESSION_KEY, null)
    return me
  } catch {
    return null
  }
}

export type SignInResult = { ok: true } | { ok: false; label: string }

export async function signIn(username: string, pin: string): Promise<SignInResult> {
  let res: Response
  try {
    res = await fetch('/api/auth/login', {
      method: 'POST',
      headers: { 'content-type': 'application/json', ...authHeaders() },
      body: JSON.stringify({ username, pin }),
    })
  } catch {
    return { ok: false, label: 'The control server is not answering.' }
  }
  const body = (await res.json().catch(() => ({}))) as { token?: string; label?: string }
  if (!res.ok || !body.token) {
    return { ok: false, label: body.label ?? 'That username and PIN don’t match.' }
  }
  write(SESSION_KEY, body.token)
  changed()
  return { ok: true }
}

export async function signOut(): Promise<void> {
  const headers = authHeaders()
  write(SESSION_KEY, null)
  changed()
  // After the local token is gone, so a server that never answers still signs
  // this browser out.
  await fetch('/api/auth/logout', { method: 'POST', headers }).catch(() => {})
}

// ---------------------------------------------------------------------------
// Reading identity from anywhere in the tree.

export const IdentityContext = createContext<Me | null>(null)

export const useIdentity = () => useContext(IdentityContext)

export function can(me: Me | null, permission: string): boolean {
  if (!me) return true // see the header: unknown answers yes, deliberately
  if (me.unconfigured) return true
  return me.permissions.includes('*') || me.permissions.includes(permission)
}

export const useCan = (permission: string) => can(useIdentity(), permission)
