/**
 * The take log — and the bridge between the two live-data paths.
 *
 * Operators want a feed: "Lower Third · taken 10:15:03.12 · frame 1841944".
 * That is event-shaped data a browser wants over SSE, which is the conflating
 * transport. The naive move is to publish {event:"take", ...} on a topic, and
 * that is exactly the trap streamHub.js warns about — a superseded event is a
 * LOST event.
 *
 * ---------------------------------------------------------------------------
 * THE RULE
 *
 *   An event stream reaches the browser as THE SNAPSHOT OF A LOG TAIL.
 *
 * Topic `show:<id>:cuelog` publishes { head_seq, entries[] } — a sliding window,
 * not one frame per event. Conflation becomes safe again because the newest
 * frame CONTAINS everything the dropped frames carried. A client that has been
 * away fetches the gap with ?since=<seq> over plain HTTP.
 *
 * This is the general escape from the trap, not a one-off for this feature.
 * ADR 0008.
 * ---------------------------------------------------------------------------
 *
 * Entries record WHICH SURFACE fired the cue. Cue authority is genuinely
 * distributed — GPI, OSC, and MIDI terminate at the node, not here (ADR 0014) —
 * so without `surface` the log is confusing the first time someone uses a GPI.
 */

import { publish, registerTopic } from './streamHub.js'

/** Entries kept in the tail published over SSE. */
const TAIL = 50
/** Entries retained for ?since=<seq> gap recovery. */
const RETAIN = 5000

const SURFACES = new Set([
  'show_view', 'companion', 'rosstalk', 'gpi', 'osc', 'midi', 'api', 'node_local',
])

const logs = new Map() // showId -> { seq, entries[] }

function log(showId) {
  let l = logs.get(showId)
  if (!l) { l = { seq: 0, entries: [] }; logs.set(showId, l) }
  return l
}

const tail = (l) => ({ head_seq: l.seq, entries: l.entries.slice(-TAIL) })

/**
 * Append a take-log entry and republish the tail.
 *
 * @param {string} showId
 * @param {object} e
 * @param {string} e.cue_id      Generated where the human acted, carried end to end.
 * @param {string} e.surface     One of SURFACES — never omit it.
 * @param {string} e.op          take | take_out | update | clear_layer | clear_all
 * @param {number} [e.fired_at_frame] The frame it actually went on, from CueAck.
 */
export function append(showId, e) {
  if (!SURFACES.has(e.surface)) {
    throw new Error(`unknown cue surface ${JSON.stringify(e.surface)}`)
  }
  const l = log(showId)
  const entry = {
    seq: ++l.seq,
    at_utc: e.at_utc ?? Date.now(),
    // Not the same as at_utc: a cue is scheduled for the first uncommitted
    // frame, so the log shows when the graphic WENT ON, not when someone asked.
    fired_at_frame: e.fired_at_frame ?? null,
    cue_id: e.cue_id,
    node: e.node ?? null,
    output: e.output ?? null,
    layer: e.layer ?? null,
    op: e.op,
    item: e.item ?? null,
    item_label: e.item_label ?? null,
    surface: e.surface,
    user: e.user ?? null,
    station: e.station ?? null,
    result: e.result ?? 'applied',
  }
  l.entries.push(entry)
  if (l.entries.length > RETAIN) l.entries.splice(0, l.entries.length - RETAIN)

  // Safe to conflate: this frame supersedes every earlier one completely.
  publish(`show:${showId}:cuelog`, tail(l))
  return entry
}

/** GET /api/cuelog?show=<id>&since=<seq>&limit=<n> — the gap-recovery path. */
export function since(showId, sinceSeq = 0, limit = 500) {
  const l = log(showId)
  return { head_seq: l.seq, entries: l.entries.filter((e) => e.seq > sinceSeq).slice(0, limit) }
}

export function registerTopics({ showExists = () => true } = {}) {
  registerTopic('show:*:cuelog', {
    valid: ([showId]) => showExists(showId),
    start: () => null,
    stop: () => {},
    snapshot: ([showId]) => tail(log(showId)),
  })
}

/** Test seam. */
export function _reset() {
  logs.clear()
}
