/**
 * PMG Link framing on the JavaScript side.
 *
 * The message *shapes* are defined once, in Rust, in `crates/pmg-proto`. This
 * file is only the codec and a few constructors — anything that looks like a
 * second definition of the protocol belongs there instead, for the same reason
 * the server never parses a SceneDoc.
 *
 * Rust's externally-tagged enums arrive as single-key maps: `{ Cue: {…} }`,
 * `{ Take: {…} }`, and bare strings for unit variants (`"Applied"`). That costs
 * a few bytes and buys frames that are legible off the wire without a schema,
 * which is what you want when debugging in a booth at 09:50 on a Sunday.
 *
 * # The one interop hazard, and why it is configured rather than discovered
 *
 * `ciborium` encodes Rust `u64` as CBOR major-type 0 with a 64-bit argument,
 * and cbor-x decodes *that* to a **BigInt** by default. So `at_utc` came back
 * as `1786487930986n`, which is not `===` any Number, does not survive
 * `JSON.stringify`, and throws when mixed with arithmetic. It is the sort of
 * thing that works in every test and then fails on the first real ack.
 *
 * `int64AsNumber` is safe for every u64 in this protocol: the largest are
 * millisecond timestamps (year ~287396 before 2^53) and frame counters (2^53
 * frames at 59.94 is longer than the universe has existed). If a genuinely
 * large integer is ever added to the protocol, this assumption is where it
 * breaks, so it is stated here rather than left implicit.
 */

import { Encoder } from 'cbor-x'

const codec = new Encoder({
  int64AsNumber: true,
  // Preserve insertion order rather than sorting; the wire is not hashed, so
  // canonical ordering buys nothing and costs legibility in a capture.
  mapsAsObjects: true,
  useRecords: false,
})

export const encode = (msg) => codec.encode(msg)
export const decode = (buf) => codec.decode(buf)

/** The single-key tag of an externally-tagged enum, with its payload. */
export function tagOf(msg) {
  if (typeof msg === 'string') return [msg, null] // unit variant
  const keys = Object.keys(msg ?? {})
  if (keys.length !== 1) return [null, null]
  return [keys[0], msg[keys[0]]]
}

export const PROTO_VER = 1
export const PING_INTERVAL_MS = 1_000
export const DEAD_AFTER_MS = 3_000
export const REPLAY_RING = 1024
export const DEFAULT_TAKE_DEADLINE_MS = 500

/** Ops with nothing to be late for carry no deadline (pmg-link.md §5.4). */
const NO_DEADLINE = new Set(['Arm', 'Preload'])
export const isDeadlineBearing = (opTag) => !NO_DEADLINE.has(opTag)
