/**
 * Write CBOR fixtures the RUST tests decode.
 *
 * The mirror of `crates/pmg-proto/tests/wire.rs::write_wire_fixture`. Both
 * directions need a fixture, because the two failures are different:
 *
 * - Rust → JS caught cbor-x decoding a u64 into a BigInt.
 * - JS → Rust caught cbor-x ENCODING an integral number above 2^32 as a
 *   float64, which serde refuses where a u64 is declared. That one is worse:
 *   it failed silently on the far side of the wire, in another language, and
 *   the node ran for minutes looking healthy while never decoding a Welcome.
 *
 * Regenerate deliberately with `npm run wire:fixtures` after a protocol change.
 */

import fs from 'node:fs'
import path from 'node:path'
import * as wire from '../server/wire.js'

const dir = path.join(import.meta.dirname, '..', 'testdata', 'proto')
fs.mkdirSync(dir, { recursive: true })

const write = (name, msg) => {
  fs.writeFileSync(path.join(dir, name), Buffer.from(wire.encode(msg)))
  console.log('wrote', name)
}

// Fixed timestamps, not Date.now(): a fixture that changes every run cannot be
// committed, and the whole point is that the bytes are checked in. The value is
// deliberately above 2^32, which is exactly where the float bug lived.
const T_MS = 1786487930986
const FRAME = 1841944

write('js-ping.cbor', { Ping: { t_ms: T_MS } })

write('js-welcome.cbor', {
  Welcome: {
    server_id: '01JSRV0000000000000000000',
    epoch: 19,
    resume_from_seq: 8813,
    time_ref: { utc_ms: T_MS, monotonic_ns: 123456789012345 },
  },
})

write('js-cue.cbor', {
  Cue: {
    cue_id: '01JAQ8Z3K7V2N5R9T1W4Y6B8C0',
    seq: 8813,
    epoch: 19,
    deadline_ms: 500,
    at: null,
    op: { Take: { item: '01JB', output: 'out-0', layer: 'Lower Third' } },
  },
})

write('js-assignment.cbor', {
  Assignment: {
    gen: 42,
    digest: 'sha256:abc',
    outputs: [{ id: 'out-0', mode: '1920x1080', layers: ['Lower Third'] }],
    templates: [{ template_rev: `blake3:${'0'.repeat(64)}`, name: 'Lower Third', min_reader: 1 }],
    items: [{
      id: '01JB', label: 'Pastor Dan', template_rev: `blake3:${'0'.repeat(64)}`,
      data: { name: { Text: 'Pastor Dan Whitfield' }, score: { Number: 3 } },
      output: 'out-0', layer: 'Lower Third',
    }],
    assets: [],
  },
})

console.log('fixture timestamp', T_MS, '- above 2^32 on purpose; frame', FRAME)
