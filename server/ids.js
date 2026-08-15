/**
 * Identifiers.
 *
 * ULID-shaped: 48 bits of millisecond timestamp then 80 bits of randomness,
 * Crockford base32, 26 characters. Lexicographic order is creation order, which
 * makes `ORDER BY id` meaningful and a rundown's ids readable in a log.
 *
 * Hand-rolled rather than a dependency because it is fifteen lines and the
 * whole family avoids dependencies it can read in one sitting.
 *
 * These are NOT content addresses. Anything named by its bytes uses
 * `blake3:<hex>` and comes from `cas.js` — the two are deliberately impossible
 * to confuse at a glance.
 */

import { randomBytes } from 'node:crypto'

const ALPHABET = '0123456789ABCDEFGHJKMNPQRSTVWXYZ' // Crockford: no I, L, O, U

function encode(bytes, chars) {
  let bits = 0n
  for (const b of bytes) bits = (bits << 8n) | BigInt(b)
  let out = ''
  for (let i = 0; i < chars; i++) {
    out = ALPHABET[Number(bits & 31n)] + out
    bits >>= 5n
  }
  return out
}

export function newId(now = Date.now()) {
  const time = Buffer.alloc(6)
  time.writeUIntBE(now, 0, 6)
  return encode(time, 10) + encode(randomBytes(10), 16)
}

export const ID_RE = /^[0-9A-HJKMNP-TV-Z]{26}$/
