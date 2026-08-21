/**
 * The permission boundary. ADR 0013.
 *
 * The claims worth defending: an unconfigured server is open and SAYS SO, the
 * first user closes it for good, 401 and 403 mean different things, and a
 * trusted station can operate within its scope and nowhere else.
 */

import assert from 'node:assert/strict'
import test, { before, beforeEach } from 'node:test'
import fs from 'node:fs'

let store, db, app, port, server

before(async () => {
  process.env.PMG_DB = ':memory:'
  process.env.PMG_CAS = fs.mkdtempSync('/tmp/pmg-auth-cas-')
  store = await import('./authStore.js')
  db = await import('./db.js')
  app = (await import('./index.js')).createApp()
  server = app.listen(0)
  await new Promise((r) => server.once('listening', r))
  port = server.address().port
})
test.after(() => server?.close())

const req = async (method, url, body, headers = {}) => {
  const res = await fetch(`http://127.0.0.1:${port}${url}`, {
    method,
    headers: { ...(body ? { 'content-type': 'application/json' } : {}), ...headers },
    ...(body ? { body: JSON.stringify(body) } : {}),
  })
  const text = await res.text()
  return { status: res.status, json: text ? JSON.parse(text) : null }
}

beforeEach(() => {
  store._reset()
  db.handle().exec(`DELETE FROM sessions; DELETE FROM user_groups; DELETE FROM group_permissions;
                    DELETE FROM users; DELETE FROM groups; DELETE FROM stations; DELETE FROM audit_log;`)
})

const admin = () => {
  const g = store.createGroup('Administrators', ['*'])
  return { group: g, user: store.createUser({ username: 'jbeale', pin: '1234', groups: [g.id] }) }
}

// ---------------------------------------------------------------------------

test('an unconfigured server is open, and says so', async () => {
  assert.equal(store.unconfigured(), true)
  const me = await req('GET', '/api/auth/me')
  // Said out loud rather than inferred, so a UI cannot present an open server
  // as a secured one.
  assert.equal(me.json.unconfigured, true)

  // And it genuinely is open — a booth appliance locked behind a password
  // nobody wrote down is an outage on Sunday.
  const r = await req('POST', '/api/playlists', { name: 'Anyone can' })
  assert.equal(r.status, 201)
})

test('creating the first user closes it', async () => {
  admin()
  assert.equal(store.unconfigured(), false)
  const r = await req('POST', '/api/playlists', { name: 'Not any more' })
  // 401, not 403: nobody is logged in. That distinction is the browser's only
  // way to tell "sign in" from "ask someone else".
  assert.equal(r.status, 401)
  assert.equal(r.json.error, 'permission_required')
  assert.match(r.json.label, /Sign in/)
})

test('the FIRST user gets the wildcard, or securing the server locks it', async () => {
  // Creating the first user turns enforcement on for good. A first user with
  // no group would turn it on and hold nothing — unable to create a group,
  // grant a permission, or add anybody else, with no way back that does not
  // involve a text editor and the database file.
  const first = store.createUser({ username: 'dan', pin: '1234' })
  assert.equal(first.granted_admin, true, 'and it says so rather than being a surprise')
  assert.ok(store.permissionsOf(first.id).has('graphics.users.manage'))

  // The SECOND user gets exactly what they were given, which is nothing.
  const second = store.createUser({ username: 'sam', pin: '4321' })
  assert.equal(second.granted_admin, false)
  assert.equal(store.permissionsOf(second.id).size, 0)

  // And a first user who WAS given a group keeps only that group.
  db._reset()
  const g = store.createGroup('Operators', ['graphics.operate'])
  const only = store.createUser({ username: 'kim', pin: '9876', groups: [g.id] })
  assert.equal(only.granted_admin, false)
  assert.equal(store.permissionsOf(only.id).has('graphics.users.manage'), false)
})

test('a logged-in user without the permission gets 403 and is told which', async () => {
  const g = store.createGroup('Operators', ['graphics.operate'])
  store.createUser({ username: 'sam', pin: '4321', groups: [g.id] })
  admin() // so the server is configured either way

  const login = await req('POST', '/api/auth/login', { username: 'sam', pin: '4321' })
  assert.equal(login.status, 200)
  const auth = { authorization: `Bearer ${login.json.token}` }

  const refused = await req('POST', '/api/playlists', { name: 'nope' }, auth)
  assert.equal(refused.status, 403, 'logged in, lacking the permission')
  // Names the authority that is missing, so the message is actionable.
  assert.match(refused.json.label, /Edit items, playlists and the rundown/)
  assert.equal(refused.json.permission, 'graphics.rundown.edit')
})

test('an administrator holds every permission through the wildcard', async () => {
  const { user } = admin()
  const perms = store.permissionsOf(user.id)
  for (const p of store.PERMISSIONS) assert.ok(perms.has(p), `missing ${p}`)
})

test('a wrong PIN and an unknown user are the same answer', async () => {
  admin()
  const wrong = await req('POST', '/api/auth/login', { username: 'jbeale', pin: '9999' })
  const nobody = await req('POST', '/api/auth/login', { username: 'ghost', pin: '9999' })
  // Distinguishing them tells an attacker which usernames exist.
  assert.equal(wrong.status, 401)
  assert.deepEqual(wrong.json, nobody.json)
})

test('repeated failures are throttled per station and username', async () => {
  admin()
  for (let i = 0; i < 5; i++) await req('POST', '/api/auth/login', { username: 'jbeale', pin: '0000' })
  const blocked = await req('POST', '/api/auth/login', { username: 'jbeale', pin: '0000' })
  assert.equal(blocked.status, 429)
  assert.match(blocked.json.label, /Try again/)

  // A different username is not affected: one jammed keypad must not lock out
  // the building.
  const other = await req('POST', '/api/auth/login', { username: 'someone-else', pin: '0000' })
  assert.equal(other.status, 401)
})

test('a session expires and stops working', async () => {
  admin()
  const login = await req('POST', '/api/auth/login', { username: 'jbeale', pin: '1234' })
  db.handle().prepare('UPDATE sessions SET expires_at = ?').run(Date.now() - 1000)
  const r = await req('POST', '/api/playlists', { name: 'stale' }, { authorization: `Bearer ${login.json.token}` })
  assert.equal(r.status, 401)
})

test('logging out invalidates the token immediately', async () => {
  admin()
  const login = await req('POST', '/api/auth/login', { username: 'jbeale', pin: '1234' })
  const auth = { authorization: `Bearer ${login.json.token}` }
  assert.equal((await req('POST', '/api/playlists', { name: 'ok' }, auth)).status, 201)
  await req('POST', '/api/auth/logout', null, auth)
  assert.equal((await req('POST', '/api/playlists', { name: 'gone' }, auth)).status, 401)
})

// ---------------------------------------------------------------------------
// The one deliberate weakening
// ---------------------------------------------------------------------------

test('a trusted station may operate with nobody logged in, within its scope', async () => {
  admin()
  const st = store.registerStation('booth-mac')
  store.setStationTrust(st.id, { trusted: true, scope: { outputs: ['out-0'], layers: ['Lower Third'] } })
  const asStation = { 'x-prodmesh-station': st.token }

  // Inside the scope: allowed, with no user. It fails later for an unrelated
  // reason (no such node), which is the point — it got PAST the boundary.
  const inScope = await req('POST', '/api/cue', {
    cue_id: 'c1', node: 'nope', output: 'out-0', layer: 'Lower Third', op: 'take_out',
  }, asStation)
  assert.notEqual(inScope.status, 401)
  assert.notEqual(inScope.status, 403)

  // Outside the scope: refused. Trust is per station and scoped to named
  // outputs and layers, never global.
  const outOfScope = await req('POST', '/api/cue', {
    cue_id: 'c2', node: 'nope', output: 'out-0', layer: 'Bug', op: 'take_out',
  }, asStation)
  assert.equal(outOfScope.status, 401)
})

test('a trusted station may still only operate, never edit', async () => {
  admin()
  const st = store.registerStation('booth-mac')
  store.setStationTrust(st.id, { trusted: true, scope: null })
  const r = await req('POST', '/api/playlists', { name: 'nope' }, { 'x-prodmesh-station': st.token })
  // It grants graphics.operate and nothing else.
  assert.equal(r.status, 401)
})

test('registering a station grants nothing by itself', async () => {
  admin()
  const st = store.registerStation('random-laptop')
  const r = await req('POST', '/api/cue', {
    cue_id: 'c3', node: 'nope', output: 'out-0', layer: 'Lower Third', op: 'take_out',
  }, { 'x-prodmesh-station': st.token })
  assert.equal(r.status, 401)
})

// ---------------------------------------------------------------------------

test('refusals are audited, not just successes', async () => {
  const { user } = admin()
  await req('POST', '/api/playlists', { name: 'nope' }) // refused, nobody logged in

  const entries = store.recentAudit(50)
  const refusal = entries.find((e) => e.result === 'refused')
  // A log that records only successes cannot answer "why couldn't they take it
  // on air?", which is the question actually asked afterwards.
  assert.ok(refusal, 'a refusal must be recorded')
  assert.equal(refusal.action, 'graphics.rundown.edit')
  assert.ok(user.id)
})

test('a station token is returned once and never stored in the clear', async () => {
  admin()
  const st = store.registerStation('booth-mac')
  const row = db.handle().prepare('SELECT * FROM stations WHERE id = ?').get(st.id)
  assert.ok(!JSON.stringify(row).includes(st.token), 'the raw token must not be in the row')
  // It still resolves, because only its hash was needed.
  assert.equal(store.stationByToken(st.token)?.id, st.id)
})

test('a PIN is never stored in the clear', () => {
  const { user } = admin()
  const row = db.handle().prepare('SELECT * FROM users WHERE id = ?').get(user.id)
  assert.ok(!row.pin_hash.includes('1234'))
  assert.notEqual(row.pin_hash, '1234')
  assert.ok(row.pin_salt.length >= 16)
})

test('an unknown permission in a group is refused rather than silently granting nothing', async () => {
  admin()
  const login = await req('POST', '/api/auth/login', { username: 'jbeale', pin: '1234' })
  const r = await req('POST', '/api/groups', { name: 'Typo', permissions: ['graphics.oprate'] },
    { authorization: `Bearer ${login.json.token}` })
  assert.equal(r.status, 422)
  assert.match(r.json.label, /Not a permission/)
})

// ---------------------------------------------------------------------------
// Naming a station, and getting back in. ADR 0025.

test('a station can be renamed, because the browser names it before a person does', async () => {
  admin()
  const login = await req('POST', '/api/auth/login', { username: 'jbeale', pin: '1234' })
  const auth = { authorization: `Bearer ${login.json.token}` }
  const st = store.registerStation('Chrome · macOS')

  const r = await req('PUT', `/api/stations/${st.id}`, { name: 'Booth Mac' }, auth)
  assert.equal(r.status, 200)
  assert.equal(r.json.name, 'Booth Mac')
  // And it is still the same station: renaming must not cost it its trust.
  assert.equal(store.stationByToken(st.token)?.id, st.id)

  const missing = await req('PUT', '/api/stations/nope', { name: 'Anything' }, auth)
  assert.equal(missing.status, 404)
})

test('renaming a station needs the authority to manage stations', async () => {
  admin()
  const st = store.registerStation('Chrome · macOS')
  const r = await req('PUT', `/api/stations/${st.id}`, { name: 'Booth Mac' })
  assert.equal(r.status, 401)
})

/**
 * The last-resort recovery path, and the reason it is worth a test: the first
 * user turns permissions on for good, so a forgotten PIN with nobody else to
 * grant anything is a server locked out of its own administration. Its
 * authority is physical access to the database, never HTTP.
 */
test('a PIN can be reset from the machine holding the database, and it ends every session', async () => {
  const { user } = admin()
  const before = await req('POST', '/api/auth/login', { username: 'jbeale', pin: '1234' })
  assert.equal(before.status, 200)

  assert.equal(store.setPin('jbeale', '5678').id, user.id)

  const stale = await req('GET', '/api/users', null, { authorization: `Bearer ${before.json.token}` })
  assert.equal(stale.status, 401, 'the session it was signed in with is gone')

  const old = await req('POST', '/api/auth/login', { username: 'jbeale', pin: '1234' })
  assert.equal(old.status, 401)
  const now = await req('POST', '/api/auth/login', { username: 'jbeale', pin: '5678' })
  assert.equal(now.status, 200)

  assert.equal(store.setPin('nobody', '5678'), null)
})

test('resetting a PIN never becomes an HTTP route', async () => {
  admin()
  const login = await req('POST', '/api/auth/login', { username: 'jbeale', pin: '1234' })
  const auth = { authorization: `Bearer ${login.json.token}` }
  // Not even an administrator: physical access to the server is the authority,
  // and an endpoint that sets a PIN is a rather different blast radius.
  for (const [method, url] of [
    ['POST', '/api/users/jbeale/pin'],
    ['PUT', '/api/users/jbeale/pin'],
    ['POST', '/api/auth/reset-pin'],
  ]) {
    // Raw, because a 404 from Express is an HTML page and req() expects JSON.
    const r = await fetch(`http://127.0.0.1:${port}${url}`, {
      method,
      headers: { 'content-type': 'application/json', ...auth },
      body: JSON.stringify({ pin: '0000' }),
    })
    assert.equal(r.status, 404, `${method} ${url} must not exist`)
  }
})

test('a group is listed with the permissions it actually holds', async () => {
  admin()
  const login = await req('POST', '/api/auth/login', { username: 'jbeale', pin: '1234' })
  const r = await req('GET', '/api/users', null, { authorization: `Bearer ${login.json.token}` })

  const admins = r.json.groups.find((g) => g.name === 'Administrators')
  // Listing Administrators as holding nothing is the most misleading row this
  // page can print, and it printed it: the query selected the group table and
  // the permissions live in another one.
  assert.deepEqual(admins.permissions, ['*'])
})
