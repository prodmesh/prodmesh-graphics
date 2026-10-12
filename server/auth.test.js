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
import { WebSocket } from 'ws'

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
  ;(await import('./cueSocket.js')).attach(server)
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

/**
 * The lockout that `createUser`'s wildcard grant does NOT cover: an account
 * made by an older build, or the only administrator taken out of the group.
 * Enforcement is on, nobody holds `graphics.users.manage`, and every route that
 * could put it right refuses. It happened for real on the rehearsal server.
 */
test('a server that has lost its administrator says so, rather than just refusing', async () => {
  // A user in no group at all — which is exactly what the rehearsal database
  // held: one account, zero groups, zero permissions.
  store.createUser({ username: 'orphan', pin: '1234' })
  db.handle().prepare('DELETE FROM user_groups').run()

  const me = await req('GET', '/api/auth/me')
  assert.equal(me.json.unconfigured, false, 'enforcement is on')
  assert.equal(me.json.no_administrator, true)

  // And it is not silently reopened. Quietly dropping enforcement because
  // permissions are misconfigured would turn a broken row into a way in.
  const refused = await req('POST', '/api/playlists', { name: 'nope' })
  assert.equal(refused.status, 401)
})

test('granting an administrator from the console puts the server right', async () => {
  store.createUser({ username: 'orphan', pin: '1234' })
  db.handle().prepare('DELETE FROM user_groups').run()
  assert.equal(store.administrable(), false)

  assert.ok(store.grantAdmin('orphan'))
  assert.equal(store.administrable(), true)
  assert.equal((await req('GET', '/api/auth/me')).json.no_administrator, false)

  // Live for a session that is ALREADY open: permissions are read per request,
  // not baked into the token.
  const login = await req('POST', '/api/auth/login', { username: 'orphan', pin: '1234' })
  const r = await req('GET', '/api/users', null, { authorization: `Bearer ${login.json.token}` })
  assert.equal(r.status, 200)

  assert.equal(store.grantAdmin('nobody'), null)
})

test('an Administrators group that has lost the wildcard is repaired, not duplicated', async () => {
  const g = store.createGroup('Administrators', [])
  store.createUser({ username: 'orphan', pin: '1234', groups: [g.id] })
  assert.equal(store.administrable(), false, 'a group named right holding nothing is the same lockout')

  store.grantAdmin('orphan')
  const groups = db.handle().prepare("SELECT id FROM groups WHERE name = 'Administrators'").all()
  assert.equal(groups.length, 1, 'the existing group is repaired')
  assert.deepEqual([...store.permissionsOf(db.handle().prepare('SELECT id FROM users WHERE username = ?').get('orphan').id)].sort(), [...store.PERMISSIONS].sort())
})

test('granting an administrator never becomes an HTTP route', async () => {
  admin()
  const login = await req('POST', '/api/auth/login', { username: 'jbeale', pin: '1234' })
  const auth = { authorization: `Bearer ${login.json.token}` }
  for (const [method, url] of [
    ['POST', '/api/users/jbeale/admin'],
    ['POST', '/api/auth/grant-admin'],
  ]) {
    const r = await fetch(`http://127.0.0.1:${port}${url}`, {
      method,
      headers: { 'content-type': 'application/json', ...auth },
      body: JSON.stringify({}),
    })
    assert.equal(r.status, 404, `${method} ${url} must not exist`)
  }
})

test('an output’s audio destination survives the server (ADR 0028)', async () => {
  admin()
  const login = await req('POST', '/api/auth/login', { username: 'jbeale', pin: '1234' })
  const auth = { authorization: `Bearer ${login.json.token}` }
  db.handle()
    .prepare("INSERT OR REPLACE INTO nodes (node_id,name,adopted,last_seen) VALUES ('n1','booth',1,0)")
    .run()

  const r = await req('PUT', '/api/nodes/n1/output-config', {
    outputs: [{
      id: 'main', backend: 'ndi', ndi_name: 'PMG', audio_channels: 2,
      audio_device: 'Dante Virtual Soundcard', audio_host: 'CoreAudio', group: 'g0',
    }],
  }, auth)
  assert.equal(r.status, 200)

  const back = await req('GET', '/api/nodes/n1/output-config', null, auth)
  const out = back.json.outputs[0]
  // Passed through, not checked against a device list: the node is the only
  // thing that knows what is plugged into it, and it refuses by name.
  assert.equal(out.audio_device, 'Dante Virtual Soundcard')
  assert.equal(out.audio_host, 'CoreAudio')
})

// ---------------------------------------------------------------------------
// The cue socket enforces the same boundary as POST /api/cue (ADR 0017)
// ---------------------------------------------------------------------------

/** Send one cue over the socket with the given hello; resolve its ack. */
async function socketCue(hello, cue) {
  const ws = new WebSocket(`ws://127.0.0.1:${port}/api/cue/socket`)
  await new Promise((r, j) => { ws.once('open', r); ws.once('error', j) })
  const ack = new Promise((r) => ws.on('message', (m) => {
    const msg = JSON.parse(String(m))
    if (msg.type === 'ack') r(msg)
  }))
  ws.send(JSON.stringify({ type: 'hello', ...hello }))
  ws.send(JSON.stringify({ type: 'cue', ...cue }))
  const a = await ack
  ws.close()
  return a
}

test('the cue socket refuses an anonymous cue once the server is secured', async () => {
  admin()
  const a = await socketCue({}, { cue_id: 's1', node: 'nope', output: 'out-0', layer: 'Lower Third', op: 'take_out' })
  // A socket is not a way round the boundary: same status, same sentence.
  assert.equal(a.http, 401)
  assert.equal(a.error, 'permission_required')
  assert.equal(a.cue_id, 's1')
})

test('the cue socket honours a signed-in user and a trusted station’s scope', async () => {
  admin()
  const login = await req('POST', '/api/auth/login', { username: 'jbeale', pin: '1234' })
  const signedIn = await socketCue({ bearer: login.json.token },
    { cue_id: 's2', node: 'nope', output: 'out-0', layer: 'Lower Third', op: 'take_out' })
  assert.ok(![401, 403].includes(signedIn.http), 'past the boundary, then no such node')

  const st = store.registerStation('booth-mac')
  store.setStationTrust(st.id, { trusted: true, scope: { outputs: ['out-0'], layers: ['Lower Third'] } })
  const inScope = await socketCue({ station: st.token },
    { cue_id: 's3', node: 'nope', output: 'out-0', layer: 'Lower Third', op: 'take_out' })
  assert.ok(![401, 403].includes(inScope.http))
  const outOfScope = await socketCue({ station: st.token },
    { cue_id: 's4', node: 'nope', output: 'out-0', layer: 'Bug', op: 'take_out' })
  assert.equal(outOfScope.http, 401)
})
