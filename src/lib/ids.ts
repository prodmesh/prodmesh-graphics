/**
 * Identifiers — the browser half of `server/ids.js`, same shape on purpose.
 *
 * ULID-shaped: 48 bits of millisecond timestamp then 80 bits of randomness,
 * Crockford base32, 26 characters. Lexicographic order is creation order.
 * Hand-rolled for the same reason as the server's: it is fifteen lines and the
 * whole family avoids dependencies it can read in one sitting.
 *
 * These are NOT content addresses. Anything named by its bytes uses
 * `blake3:<hex>` and comes from the CAS.
 */

const ALPHABET = '0123456789ABCDEFGHJKMNPQRSTVWXYZ' // Crockford: no I, L, O, U

function encode(bytes: Uint8Array, chars: number): string {
  let bits = 0n
  for (const b of bytes) bits = (bits << 8n) | BigInt(b)
  let out = ''
  for (let i = 0; i < chars; i++) {
    out = ALPHABET[Number(bits & 31n)] + out
    bits >>= 5n
  }
  return out
}

export function newId(now = Date.now()): string {
  const time = new Uint8Array(6)
  let t = now
  for (let i = 5; i >= 0; i--) {
    time[i] = t % 256
    t = Math.floor(t / 256)
  }
  return encode(time, 10) + encode(crypto.getRandomValues(new Uint8Array(10)), 16)
}
