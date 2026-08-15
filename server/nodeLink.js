/**
 * PMG Link — the server end. Spec: `docs/specs/pmg-link.md`. ADR 0007, 0009.
 *
 * The node dials us. One WebSocket at `/link`, CBOR frames. Assets and preview
 * are deliberately *not* here: a 4 GB video must not head-of-line-block a take,
 * and a preview stall must not delay a cue. That split is structural, and it is
 * the mitigation for most of R8.
 *
 * # What this module is not allowed to believe
 *
 * **It does not know what is on air.** The node does. `StateReport` arrives and
 * is *adopted*; there is no code path here that pushes our idea of on-air state
 * down to a node, and adding one would break the invariant a server restart is
 * supposed to hold (ADR 0009).
 *
 * # epoch and boot_id
 *
 * `epoch` is new every server start; `boot_id` is new every node start.
 * Together they distinguish the four cases in §2.2, which need different
 * answers — "the node restarted" and "the server restarted" are not the same
 * event and treating them alike loses either cues or on-air state.
 */

import { WebSocketServer } from 'ws'
import { randomUUID } from 'node:crypto'
import { publish, registerTopic } from './streamHub.js'
import * as wire from './wire.js'
import { build as buildAssignment } from './assignment.js'

/** New every server start. Cue idempotency keys are scoped to it. */
const EPOCH = Math.floor(Date.now() / 1000)
const SERVER_ID = randomUUID()

/** node_id -> session */
const sessions = new Map()

let wss = null

// ---------------------------------------------------------------------------

function session(nodeId) {
  let s = sessions.get(nodeId)
  if (!s) {
    s = {
      nodeId,
      name: nodeId,
      ws: null,
      bootId: null,
      version: null,
      capabilities: null,
      connectedAt: null,
      lastSeen: 0,
      // Assignment we have offered, and what the node has acked.
      gen: 0,
      assignment: null,
      ackedGen: null,
      // Cue sequencing, per epoch.
      seq: 0,
      ring: [],
      pending: new Map(), // cue_id -> {resolve, timer}
      // Adopted verbatim from the node. Never written from our side.
      state: null,
      ready: null,
      staging: null,
    }
    sessions.set(nodeId, s)
  }
  return s
}

const isOnline = (s) => Boolean(s.ws && s.ws.readyState === 1)

/**
 * Find a session by node id or by display name.
 *
 * **Never creates one.** `session()` is the create-on-miss accessor and it is
 * only correct on the handshake path, where a node is introducing itself. Using
 * it to look up a cue target turns a wrong node reference into a brand-new
 * offline session — so a typo reports "booth-1 is offline. Anything already on
 * air is still on air.", which is a confident, reassuring, completely wrong
 * sentence. That is worse than an error.
 *
 * Names are accepted because that is what a human types and what a Companion
 * button is configured with. Ids stay the identity; the name is an alias.
 */
function find(ref) {
  if (!ref) return null
  const byId = sessions.get(ref)
  if (byId) return byId
  const named = [...sessions.values()].filter((s) => s.name === ref)
  if (named.length === 1) return named[0]

  // A name can collide: a node reinstalled with a fresh identity leaves its old
  // session behind, and both answer to the same display name. When exactly one
  // of them is actually connected, that is unambiguously the one the operator
  // means — refusing here would say "there is no node called booth-1" while
  // booth-1 sits there online.
  const reachable = named.filter(isOnline)
  if (reachable.length === 1) return reachable[0]

  // Two live nodes with one name is a genuine ambiguity, and picking one would
  // be a coin flip deciding what goes on air.
  return null
}

function send(s, msg) {
  if (!isOnline(s)) return false
  s.ws.send(wire.encode(msg))
  return true
}

/** Publish the snapshot topics this node feeds. All conflate safely. */
function republish(s) {
  publish(`node:${s.nodeId}:health`, {
    node: s.nodeId,
    name: s.name,
    reachable: isOnline(s),
    version: s.version,
    assignment_gen: s.ackedGen,
    last_seen: s.lastSeen,
    epoch: EPOCH,
  })
  if (s.state) {
    for (const out of s.state.outputs ?? []) {
      publish(`output:${out.id}:layers`, {
        output: out.id,
        node: s.nodeId,
        fps: out.fps,
        late_frames: out.late_frames,
        layers: out.layers ?? [],
      })
    }
  }
  if (s.ready) {
    for (const id of s.ready.items_ready ?? []) {
      publish(`item:${id}:readiness`, { item: id, ready: true, missing: [] })
    }
    for (const b of s.ready.items_blocked ?? []) {
      publish(`item:${b.item}:readiness`, {
        item: b.item,
        ready: false,
        missing: b.missing ?? [],
        unreadable: b.unreadable ?? null,
      })
    }
  }
  if (s.staging) publish(`node:${s.nodeId}:staging`, { node: s.nodeId, ...s.staging })
}

// ---------------------------------------------------------------------------
// Handshake
// ---------------------------------------------------------------------------

function onHello(ws, hello) {
  const s = session(hello.node_id)
  const sameBoot = s.bootId === hello.boot_id

  // §2.2. The node kept its boot id and knows our epoch, so it has been here
  // this run: replay from where it stopped. Anything else is a full resync —
  // and note that "the server restarted" is the case where the node has been
  // happily playing all along, which is exactly why we ask it for state rather
  // than telling it any.
  const resumeFrom = sameBoot && hello.last_applied_seq < s.seq ? hello.last_applied_seq + 1 : null

  s.ws = ws
  s.bootId = hello.boot_id
  s.name = hello.name || s.nodeId
  s.version = hello.version
  s.capabilities = hello.capabilities
  s.connectedAt = Date.now()
  s.lastSeen = Date.now()
  if (!sameBoot) {
    // The node restarted. Its idea of applied cues is gone with it, and any
    // ack we are still waiting on will never arrive.
    s.seq = 0
    s.ring = []
    s.ackedGen = null
    for (const p of s.pending.values()) p.settle({ result: 'rejected', reason: 'node_restarted' })
    s.pending.clear()
  }

  send(s, {
    Welcome: {
      server_id: SERVER_ID,
      epoch: EPOCH,
      resume_from_seq: resumeFrom,
      // Number(), not Math.round(): process.hrtime.bigint() is a BigInt and
      // Math.round throws on one. Thrown here it would die inside the ws
      // message handler, where the only symptom is a node that connects and
      // is never welcomed.
      time_ref: { utc_ms: Date.now(), monotonic_ns: Number(process.hrtime.bigint()) },
    },
  })

  // Offer the current content on every connect. The node answers
  // AssignmentNeed or AssignmentOk, so an already-current node costs one
  // round trip and no transfer — which is the whole reason the offer carries a
  // digest instead of the body.
  try {
    offerAssignment(hello.node_id, buildAssignment({ nodeId: hello.node_id }))
  } catch (e) {
    // A node that cannot be given content is still a node that may be holding
    // graphics on air. Log and carry on rather than dropping the link.
    console.error('link: could not build an assignment', e)
  }
  republish(s)
  return s
}

// ---------------------------------------------------------------------------
// Frames from the node
// ---------------------------------------------------------------------------

function onFrame(ws, buf, bound) {
  let msg
  try {
    msg = wire.decode(buf)
  } catch {
    // A frame we cannot parse is not a reason to drop a link that may be
    // holding graphics on air.
    return bound
  }
  const [tag, body] = wire.tagOf(msg)

  if (tag === 'Hello') return onHello(ws, body)
  const s = bound
  if (!s) return null // nothing before Hello
  s.lastSeen = Date.now()

  switch (tag) {
    case 'AssignmentNeed':
      if (s.assignment && body.gen === s.gen) send(s, { Assignment: s.assignment })
      break

    case 'AssignmentOk':
    case 'AssignmentAck':
      // The node has this durably on disk. Only now is it true that it could
      // reconstruct this without us — which is the whole autonomy invariant.
      if (body.gen === s.gen) s.ackedGen = body.gen
      republish(s)
      break

    case 'StateReport':
      s.state = body
      republish(s)
      break

    case 'SyncProgress':
      s.staging = body
      republish(s)
      break

    case 'Ready':
      s.ready = body
      republish(s)
      break

    case 'CueAck': {
      const p = s.pending.get(body.cue_id)
      if (p) {
        s.pending.delete(body.cue_id)
        p.settle({
          result: String(body.result ?? 'applied').toLowerCase(),
          reason: body.reason ? String(body.reason) : null,
          fired_at_frame: body.fired_at_frame ?? null,
          at_utc: body.at_utc ?? Date.now(),
          seq: body.seq,
        })
      }
      break
    }

    case 'CueGap': {
      // Replay from the ring, or admit we cannot and say so. Silently skipping
      // cues would leave the node's idea of a layer permanently wrong.
      const replay = s.ring.filter((c) => c.seq >= body.from && c.seq <= body.to)
      if (replay.length !== body.to - body.from + 1) {
        send(s, { Resync: { reason: `gap ${body.from}-${body.to} exceeds the replay ring` } })
      } else {
        for (const c of replay) send(s, { Cue: c })
      }
      break
    }

    case 'Pong':
      break
  }
  return s
}

// ---------------------------------------------------------------------------
// Public surface
// ---------------------------------------------------------------------------

export function attach(server) {
  wss = new WebSocketServer({ server, path: '/link' })
  wss.on('connection', (ws) => {
    let bound = null
    ws.on('message', (buf) => {
      // A throw in here is invisible: ws swallows it and the node simply never
      // hears back. Log it and keep the link, which may be holding graphics on
      // air.
      try {
        bound = onFrame(ws, buf, bound)
      } catch (e) {
        console.error(`link: frame from ${bound?.nodeId ?? 'unhandshaked node'}`, e)
      }
    })
    const stop = () => {
      if (bound && bound.ws === ws) {
        bound.ws = null
        // Everything it had on air is STILL ON AIR. Losing the link is not
        // losing the graphic, and the UI must say the first thing rather than
        // imply the second.
        republish(bound)
      }
    }
    ws.on('close', stop)
    ws.on('error', stop)
  })

  const ping = setInterval(() => {
    for (const s of sessions.values()) {
      if (isOnline(s)) send(s, { Ping: { t_ms: Date.now() } })
      else if (s.lastSeen) republish(s)
    }
  }, wire.PING_INTERVAL_MS)
  ping.unref?.()
  return wss
}

export function offerAssignment(nodeId, assignment) {
  const s = find(nodeId)
  if (!s) return null
  s.gen += 1
  s.assignment = { ...assignment, gen: s.gen }
  s.ackedGen = null
  send(s, { AssignmentOffer: { gen: s.gen, digest: s.assignment.digest } })
  return s.gen
}

/**
 * Send a cue and **wait for the node to confirm it**.
 *
 * This is the synchronous edge, and it is the reason `POST /api/cue` blocks.
 * A fire-and-forget button on a broadcast system is worse than no button,
 * because it teaches an operator to trust something untrue.
 *
 * Resolves `{ result, reason, fired_at_frame, at_utc }`, where result is
 * `applied` | `deduped` | `rejected` | `not_acked` | `offline`.
 */
export function cue(nodeId, { cue_id, op, deadline_ms, at = null }) {
  const s = find(nodeId)
  if (!s) {
    return Promise.resolve({ result: 'unknown_node', reason: 'no_such_node' })
  }
  if (!isOnline(s)) {
    return Promise.resolve({ result: 'offline', reason: 'node_offline' })
  }

  const [opTag] = wire.tagOf(op)
  const deadline = deadline_ms ?? (wire.isDeadlineBearing(opTag) ? wire.DEFAULT_TAKE_DEADLINE_MS : null)

  const frame = { cue_id, seq: ++s.seq, epoch: EPOCH, deadline_ms: deadline, at, op }
  s.ring.push(frame)
  if (s.ring.length > wire.REPLAY_RING) s.ring.shift()

  return new Promise((resolve) => {
    let done = false
    const settle = (v) => {
      if (done) return
      done = true
      clearTimeout(timer)
      s.pending.delete(cue_id)
      resolve(v)
    }
    // We stop waiting a little after the node's own deadline. The node is the
    // one that must refuse a late take; this timer only decides how long the
    // operator's button stays un-answered, and answering "it did not confirm"
    // before the node has finished deciding would be a lie in the other
    // direction.
    const timer = setTimeout(
      () => settle({ result: 'not_acked', reason: 'no_ack_before_deadline' }),
      (deadline ?? wire.DEFAULT_TAKE_DEADLINE_MS) + 250,
    )
    timer.unref?.()
    s.pending.set(cue_id, { settle })
    send(s, { Cue: frame })
  })
}

export function list() {
  // Reachable first, then most recently seen. A caller taking the first entry
  // should get a node that can actually do something.
  return [...sessions.values()]
    .toSorted((a, b) => Number(isOnline(b)) - Number(isOnline(a)) || b.lastSeen - a.lastSeen)
    .map((s) => ({
    node_id: s.nodeId,
    name: s.name,
    reachable: isOnline(s),
    version: s.version,
    assignment_gen: s.ackedGen,
    last_seen: s.lastSeen,
    capabilities: s.capabilities,
    state: s.state,
  }))
}

export function get(nodeId) {
  const s = find(nodeId)
  return s ? list().find((n) => n.node_id === s.nodeId) : null
}

export const epoch = () => EPOCH

export function registerTopics() {
  const known = (id) => sessions.has(id)
  registerTopic('node:*:health', {
    valid: ([id]) => known(id),
    start: () => null, stop: () => {},
    snapshot: ([id]) => {
      const s = sessions.get(id)
      return s ? { node: id, name: s.name, reachable: isOnline(s), version: s.version,
                   assignment_gen: s.ackedGen, last_seen: s.lastSeen, epoch: EPOCH } : null
    },
  })
  registerTopic('node:*:staging', {
    valid: ([id]) => known(id),
    start: () => null, stop: () => {},
    snapshot: ([id]) => ({ node: id, ...sessions.get(id)?.staging }),
  })
  registerTopic('output:*:layers', {
    // Valid only if some node actually reports this output. Subscribing starts
    // work, so accepting any string would be a resource-exhaustion primitive.
    valid: ([id]) => [...sessions.values()].some((s) => (s.state?.outputs ?? []).some((o) => o.id === id)),
    start: () => null, stop: () => {},
    snapshot: ([id]) => {
      for (const s of sessions.values()) {
        const out = (s.state?.outputs ?? []).find((o) => o.id === id)
        if (out) return { output: id, node: s.nodeId, fps: out.fps, late_frames: out.late_frames, layers: out.layers ?? [] }
      }
      return null
    },
  })
  registerTopic('item:*:readiness', {
    valid: () => true,
    start: () => null, stop: () => {},
    snapshot: ([id]) => {
      for (const s of sessions.values()) {
        if (s.ready?.items_ready?.includes(id)) return { item: id, ready: true, missing: [] }
        const b = s.ready?.items_blocked?.find((x) => x.item === id)
        if (b) return { item: id, ready: false, missing: b.missing ?? [], unreadable: b.unreadable ?? null }
      }
      return { item: id, ready: false, missing: [], unreadable: null }
    },
  })
}

/** Test seam. */
export function _reset() {
  for (const s of sessions.values()) {
    for (const p of s.pending.values()) p.settle({ result: 'rejected', reason: 'reset' })
    s.ws?.close?.()
  }
  sessions.clear()
  wss?.close?.()
  wss = null
}
