/**
 * Reset a user's PIN from the machine holding the database.
 *
 *     npm run reset-pin -- <username> <new-pin>
 *
 * # Why this exists
 *
 * Creating the first user turns permissions on for good, and that user is made
 * an Administrator so that securing the server does not lock everyone out of
 * administering it (ADR 0013, `authStore.createUser`). One case is still
 * unrecoverable: the first user forgets the PIN. There is nobody left who can
 * grant anything, no route that will accept the change, and the only way back
 * is a text editor and a scrypt hash.
 *
 * So the recovery path is deliberate rather than accidental, and its authority
 * is physical access to the server — the same reasoning ADR 0013 gives for a
 * booth being a controlled space. It is never reachable over HTTP.
 *
 * Every session the user holds is dropped, because the reason to run this is
 * either "we rotated it" or "they should not be signed in any more".
 */

import { open, handle } from '../server/db.js'
import { setPin } from '../server/authStore.js'

const [username, pin] = process.argv.slice(2)

const die = (msg) => {
  console.error(msg)
  process.exit(1)
}

if (!username || !pin) die('Usage: npm run reset-pin -- <username> <new-pin>')
if (String(pin).length < 4) die('A PIN needs at least four digits.')

const db = open()

const known = handle().prepare('SELECT username FROM users ORDER BY username').all()
if (known.length === 0) {
  db.close()
  die('This server has no users. It is open to anyone on the network — add the first user in Configure.')
}

const changed = setPin(username, String(pin))
if (!changed) {
  db.close()
  die(`There is no user called ${username}. This server has: ${known.map((u) => u.username).join(', ')}`)
}

db.close()
console.log(`${username}'s PIN is set. Any session they had is signed out.`)
