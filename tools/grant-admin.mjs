/**
 * Make a user an administrator, from the machine holding the database.
 *
 *     npm run grant-admin -- <username>
 *
 * # Why this exists
 *
 * `createUser` grants the FIRST user the wildcard so that securing a server
 * does not lock it (ADR 0013). That guard only protects accounts created by a
 * server that has it. Two ways past it are real:
 *
 *   - an account created by an older build, before the guard existed;
 *   - the only administrator removed from the group, by hand or by accident.
 *
 * Either leaves a server with enforcement ON and nobody able to grant anything.
 * Every route refuses — including the ones that would put it right — and the
 * operator experiences it as being refused everywhere for no visible reason.
 *
 * Like `reset-pin`, there is no HTTP route to this and there must not be.
 * Physical access to the database is the authority, the same reasoning ADR 0013
 * gives for a booth being a controlled space.
 */

import { open, handle } from '../server/db.js'
import { grantAdmin, permissionsOf } from '../server/authStore.js'

const [username] = process.argv.slice(2)

const die = (msg) => {
  console.error(msg)
  process.exit(1)
}

const db = open()
const known = handle().prepare('SELECT id, username FROM users ORDER BY username').all()

if (!username) {
  console.error('Usage: npm run grant-admin -- <username>')
  if (known.length) {
    console.error('\nThis server has:')
    for (const u of known) {
      const held = permissionsOf(u.id)
      console.error(`  ${u.username} — ${held.size ? `${held.size} permissions` : 'NO permissions'}`)
    }
  } else {
    console.error('\nThis server has no users. It is open to anyone on the network.')
  }
  db.close()
  process.exit(1)
}

const granted = grantAdmin(username)
if (!granted) {
  db.close()
  die(`There is no user called ${username}. This server has: ${known.map((u) => u.username).join(', ')}`)
}

db.close()
// Permissions are read from the database on every request, so a session that
// is already open picks this up on its next one. The browser still has an old
// answer for what to SHOW, which a reload fixes.
console.log(`${username} is an Administrator. It takes effect on their next request — reload the browser.`)
