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
import { desiredOutputs } from './routes/nodeOutputs.js'
import { handle } from './db.js'

/** The node's MJPEG preview port. Family 85xx: control 8519, node 8520. */
const PREVIEW_PORT = 8521

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
      // Whether an operator has said this node belongs here. Persisted, because
      // adoption must survive a server restart — otherwise every restart turns
      // the whole fleet back into strangers.
      adopted: false,
      // Where the node is, as seen from here. Used only to build the preview
      // URL: preview is a separate connection on a separate port, so the
      // browser talks to the node directly rather than through this server
      // (ADR 0007). A 4 GB-per-minute video stream must not transit the
      // control plane.
      address: null,
    }
    sessions.set(nodeId, s)
  }
  return s
}

const isOnline = (s) => Boolean(s.ws && s.ws.readyState === 1)

/**
 * What every node together says about one Item, as one answer.
 *
 * **Ready means at least one destination is ready** — ADR 0018's deliberate
 * relaxation of ADR 0006. All-or-nothing was right when an Item had one
 * destination and wrong the moment it had two: one un-staged asset on a
 * secondary feed would grey out the button for the feed that is actually going
 * out. ADR 0006's real promise — never put a half-loaded graphic on air — still
 * holds, enforced per destination by the node, which refuses to arm one whose
 * template it does not have.
 *
 * `partial` is the state worth naming: armable, and not everywhere. The
 * interface has to say so BEFORE the take, because afterwards is just the
 * aggregated ack telling the operator something they could have known.
 */
export function readinessOf(id) {
  const destinations = []
  for (const s of sessions.values()) {
    const i = (s.ready?.items ?? []).find((x) => x.item === id)
    if (!i) continue
    for (const d of i.destinations ?? []) {
      destinations.push({
        node: s.nodeId,
        output: d.output,
        layer: d.layer,
        ready: !!d.ready,
        missing: d.missing ?? [],
        unreadable: d.unreadable ?? null,
      })
    }
  }

  // No node has reported on it at all. Not ready, and deliberately without a
  // reason: "we have not heard" is not the same as "something is missing", and
  // inventing a cause here would put a wrong sentence under the button.
  if (!destinations.length) {
    return { item: id, ready: false, partial: false, missing: [], unreadable: null, destinations: [] }
  }

  const blocked = destinations.filter((d) => !d.ready)
  return {
    item: id,
    ready: destinations.some((d) => d.ready),
    partial: destinations.some((d) => d.ready) && blocked.length > 0,
    // The union over what is actually outstanding, so the existing
    // "still copying · N file(s)" count stays true rather than counting a
    // ready destination's satisfied requirements.
    missing: [...new Set(blocked.flatMap((d) => d.missing))],
    unreadable: blocked.find((d) => d.unreadable)?.unreadable ?? null,
    destinations,
  }
}

/**
 * The address a node dialled us from, which is the address a browser on the
 * same network can reach its preview at.
 *
 * `ws._socket` is the documented escape hatch in the `ws` library for reaching
 * the underlying net.Socket; there is no public accessor. IPv4-mapped IPv6
 * (`::ffff:10.0.0.5`) is unwrapped so the URL is one a browser will accept.
 */
function remoteAddressOf(ws) {
  // oxlint-disable-next-line no-underscore-dangle
  const socket = ws._socket
  return socket?.remoteAddress?.replace(/^::ffff:/, '') ?? null
}

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

/**
 * Record the outputs a node reports.
 *
 * **Only ones the server has never seen.** Configure is authoritative for an
 * output that already exists — overwriting an operator's layer stack every time
 * a node reconnects would undo their work on a reboot. But an output that is
 * NEW has no work to undo, and not learning it was a silent trap: a node
 * reconfigured to add an output (ADR 0024) reported it, the server never
 * recorded it, and every destination naming it was quietly filtered out of
 * every cue — a take that went nowhere with nothing to say about why.
 */
function seedOutputs(s) {
  const outs = s.state?.outputs ?? []
  if (!outs.length) return
  const db = handle()
  const known = new Set(
    db
      .prepare('SELECT output_id FROM outputs WHERE node_id = ?')
      .all(s.nodeId)
      .map((r) => r.output_id),
  )

  const ins = db.prepare(
    `INSERT OR IGNORE INTO outputs (node_id, output_id, mode, layers, updated_at)
     VALUES (?, ?, ?, ?, ?)`,
  )
  const now = Date.now()
  const learned = []
  for (const o of outs) {
    if (known.has(o.id)) continue
    // The layer NAMES, in the node's order, because layer order is compositing
    // order and the node is the one that knows it.
    const layers = (o.layers ?? []).map((l) => l.name)
    if (!layers.length) continue
    ins.run(s.nodeId, o.id, o.mode ?? '1920x1080', JSON.stringify(layers), now)
    learned.push(o.id)
  }
  if (!learned.length) return
  // Said out loud, because it is a configuration decision the server made and
  // no operator did.
  console.log(`link: learned ${s.name}'s outputs — ${learned.join(', ')}`)
}

/** Publish the snapshot topics this node feeds. All conflate safely. */
/**
 * The named timelines a revision offers — `Update`, `Highlight` — from its
 * manifest, minus the reserved `In`/`Out` the take buttons already own.
 *
 * Keyed by the OCCUPANT's pinned rev, not the template's head: the buttons
 * must describe the document that is actually on air, and those can differ the
 * moment a designer publishes mid-service. Cached forever because a
 * template_rev is a content hash — the manifest behind it can never change.
 */
const timelinesByRev = new Map()
function namedTimelines(rev) {
  if (!rev) return []
  const hit = timelinesByRev.get(rev)
  if (hit) return hit
  const row = handle()
    .prepare('SELECT manifest FROM template_revisions WHERE template_rev = ?')
    .get(rev)
  if (!row) return [] // not cached: the rev may simply not be published YET
  let names = []
  try {
    names = (JSON.parse(row.manifest).timelines ?? []).filter((n) => n !== 'In' && n !== 'Out')
  } catch {
    // An unreadable manifest offers no buttons; the graphic still plays.
  }
  timelinesByRev.set(rev, names)
  return names
}

/** The `output:*:layers` payload, with each occupant told what it can do. */
function layersPayload(nodeId, out) {
  return {
    output: out.id,
    node: nodeId,
    fps: out.fps,
    late_frames: out.late_frames,
    layers: (out.layers ?? []).map((l) =>
      l.occupant
        ? { ...l, occupant: { ...l.occupant, timelines: namedTimelines(l.occupant.template_rev) } }
        : l,
    ),
  }
}

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
      publish(`output:${out.id}:layers`, layersPayload(s.nodeId, out))
    }
  }
  // Republish every Item this node mentioned, aggregated across ALL nodes.
  // Publishing this node's own view would make the last report win, and with
  // two nodes that is a coin flip rather than an answer.
  for (const i of s.ready?.items ?? []) {
    publish(`item:${i.item}:readiness`, readinessOf(i.item))
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

  // Remember the node, and recover whether it was adopted. A node the operator
  // adopted last week must not have to be adopted again because the server was
  // restarted.
  const db = handle()
  const known = db.prepare('SELECT adopted FROM nodes WHERE node_id = ?').get(hello.node_id)
  s.adopted = Boolean(known?.adopted)
  db.prepare(
    `INSERT INTO nodes (node_id, name, adopted, last_seen, capabilities)
     VALUES (?, ?, ?, ?, ?)
     ON CONFLICT(node_id) DO UPDATE SET
       name = excluded.name, last_seen = excluded.last_seen, capabilities = excluded.capabilities`,
  ).run(hello.node_id, hello.name || hello.node_id, s.adopted ? 1 : 0, Date.now(),
        JSON.stringify(hello.capabilities ?? {}))

  s.ws = ws
  // The address the node dialled us FROM, which is the address a browser on
  // the same network can reach it at.
  s.address = remoteAddressOf(ws)
  // Reported, not assumed. A node started with --api-port elsewhere used to be
  // unreachable from anything the server drew, and the preview link pointed at
  // a port that might be switched off.
  s.apiPort = hello.api_port ?? 8520
  s.previewPort = hello.preview_port ?? PREVIEW_PORT
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

  // **An unadopted node gets nothing.** It is welcomed, it appears in
  // Configure as a stranger asking to join, and that is all. Handing content to
  // any process that dials the port would make "adopt" a decoration — and on a
  // shared church network, a laptop running a stale build is exactly the thing
  // that must not quietly start receiving this week's graphics.
  try {
    if (s.adopted) {
      // Offer the current content on every connect. The node answers
      // AssignmentNeed or AssignmentOk, so an already-current node costs one
      // round trip and no transfer — which is the whole reason the offer
      // carries a digest instead of the body.
      offerAssignment(hello.node_id, buildAssignment({ nodeId: hello.node_id }))
      // And what an operator asked this node to drive (ADR 0024). Re-sent on
      // every connect, because a node reconfigured while it was off has no
      // other way to learn — and because a node whose own file already agrees
      // applies nothing and says so.
      const desired = desiredOutputs(hello.node_id)
      if (desired.length) setOutputs(hello.node_id, desired)
    } else {
      console.log(`link: ${hello.name} (${hello.node_id}) is waiting to be adopted`)
    }
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

    case 'OutputsAck':
      // What it applied, not what it was asked for.
      s.outputsAck = body
      if (body.problems?.length) {
        console.error(`link: ${s.name} could not apply outputs:`, body.problems.join('; '))
      }
      break

    case 'StateReport':
      s.state = body
      // Learn what outputs this node actually has, if nobody has said yet.
      //
      // `assignment.build()` falls back to a single output called `out-0` for a
      // node with no rows in `outputs`, so that a node is usable the moment it
      // connects. That default is right for a node running the default output
      // and WRONG for any node started with `--output id=…`: the assignment
      // names out-0, the node refuses it as an output it does not have, and the
      // node sits there linked, healthy, adopted, and holding nothing. It says
      // so in its log and nowhere else.
      //
      // The node is the authority on what it is driving (ADR 0009), so the
      // honest default is to take its word rather than invent a name. An
      // operator can still change the layer stack in Configure; this only fills
      // in what was never configured.
      seedOutputs(s)
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

/**
 * Re-offer current content to every adopted node whose assignment CHANGED.
 *
 * Called after content mutations (items, publishes) — without this, a node
 * adopted this morning plays this morning's rundown until something re-adopts
 * it, and the Media Browser's "required" column reads yesterday. The digest
 * short-circuit matters: most edits touch one node's content, and bumping
 * every session's gen for an identical assignment would churn acks for
 * nothing.
 */
export function reofferAll() {
  for (const s of sessions.values()) {
    if (!s.adopted) continue
    let built
    try {
      built = buildAssignment({ nodeId: s.nodeId })
    } catch (e) {
      console.error('link: could not rebuild content for', s.nodeId, e)
      continue
    }
    if (s.assignment?.digest === built.digest) continue
    offerAssignment(s.nodeId, built)
  }
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
 * `applied` | `deduped` | `superseded` | `rejected` | `not_acked` | `offline`.
 */
/**
 * Ask a node to reconfigure its outputs (ADR 0024).
 *
 * The node persists before it acks and answers with what it ACTUALLY applied,
 * so a device that will not open comes back as a problem rather than as
 * silence. `gen` counts up per node, which is how an ack is matched to the
 * request it answers.
 */
export function setOutputs(nodeId, outputs, { force = false } = {}) {
  const s = find(nodeId)
  if (!s) return null
  s.outputsGen = (s.outputsGen ?? 0) + 1
  const ok = send(s, {
    SetOutputs: { gen: s.outputsGen, outputs, force },
  })
  // An offline node is not a failure: the configuration is stored, and it is
  // re-sent the moment the node connects. That is the whole reason the server
  // keeps a copy of something the node owns.
  return { gen: s.outputsGen, sent: ok }
}

/** The last thing a node said about its outputs, or null. */
export function outputsAck(nodeId) {
  return find(nodeId)?.outputsAck ?? null
}

export function cue(nodeId, { cue_id, op, deadline_ms, at = null }) {
  const s = find(nodeId)
  if (!s) {
    // A node the server knows and simply cannot reach is OFFLINE, not unknown.
    // Sessions are live-only, so without this check every powered-off node
    // answers "there is no node called that — check the name in Configure",
    // which sends an operator to fix a name that was never wrong. The
    // distinction is the whole point of having two words for it.
    const known = handle()
      .prepare('SELECT 1 FROM nodes WHERE node_id = ? OR name = ?')
      .get(nodeId, nodeId)
    return Promise.resolve(
      known
        ? { result: 'offline', reason: 'node_offline' }
        : { result: 'unknown_node', reason: 'no_such_node' },
    )
  }
  if (!s.adopted) {
    // It is connected and it is not ours. Refusing here rather than at the node
    // keeps the boundary in one place.
    return Promise.resolve({ result: 'not_adopted', reason: 'not_adopted' })
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
    adopted: s.adopted,
    version: s.version,
    assignment_gen: s.ackedGen,
    last_seen: s.lastSeen,
    capabilities: s.capabilities,
    state: s.state,
    // The address the node dialled us FROM, shown plainly. It used to exist
    // only inside `preview_url`, which meant an operator debugging a node
    // could not read its IP off the screen it was already looking at.
    address: s.address ?? null,
    api_port: s.apiPort ?? null,
    // Null until the node has connected at least once — there is nowhere to
    // point at before then. Built from the port the NODE reported, so a
    // preview that was switched off does not get a link that 404s.
    preview_url:
      s.address && s.previewPort ? `http://${s.address}:${s.previewPort}/` : null,
    // The surface that still works when THIS link does not (ADR 0014) — which
    // is exactly when somebody needs it.
    status_url: s.address && s.apiPort ? `http://${s.address}:${s.apiPort}/status` : null,
    // What the node said about the last output configuration it was asked for
    // — including what it refused and what would not open (ADR 0024).
    outputs_ack: s.outputsAck ?? null,
  }))
}

/**
 * Per-node asset possession, DERIVED — a node never reports a cache listing.
 *
 * What it does report is enough: the assignment names every hash the node
 * must hold, readiness names the ones still outstanding per destination, and
 * `SyncProgress` carries the transfer aggregate. Required minus missing is
 * held. `missing` is null (unknown, not empty) until the node has reported
 * against the assignment we actually sent — "we have not heard" and "nothing
 * is missing" must never render as the same chip.
 */
export function assetSync() {
  return [...sessions.values()]
    .filter((s) => s.adopted)
    .map((s) => {
      const current = s.ready && s.ready.assignment_gen === s.gen
      return {
        node_id: s.nodeId,
        name: s.name,
        reachable: isOnline(s),
        reported: !!current,
        required: s.assignment?.assets ?? [],
        missing: current
          ? [...new Set(
              (s.ready.items ?? []).flatMap((i) =>
                (i.destinations ?? []).flatMap((d) => d.missing ?? [])),
            )]
          : null,
        staging: s.staging ?? null,
      }
    })
}

/**
 * Adopt (or release) a node.
 *
 * Adopting immediately offers it the current content — an operator who clicks
 * Adopt expects the node to be usable, not to need a restart.
 */
export function setAdopted(nodeId, adopted) {
  const s = find(nodeId)
  if (!s) return null
  s.adopted = adopted
  handle().prepare('UPDATE nodes SET adopted = ? WHERE node_id = ?').run(adopted ? 1 : 0, s.nodeId)
  if (adopted) {
    try {
      offerAssignment(s.nodeId, buildAssignment({ nodeId: s.nodeId }))
    } catch (e) {
      console.error('link: could not offer content after adoption', e)
    }
  }
  republish(s)
  return get(s.nodeId)
}

export function get(nodeId) {
  const s = find(nodeId)
  return s ? list().find((n) => n.node_id === s.nodeId) : null
}

export const epoch = () => EPOCH
export const serverId = () => SERVER_ID

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
        if (out) return layersPayload(s.nodeId, out)
      }
      return null
    },
  })
  registerTopic('item:*:readiness', {
    valid: () => true,
    start: () => null, stop: () => {},
    snapshot: ([id]) => readinessOf(id),
  })
}

/** Test seam. */
export function _reset() {
  for (const s of sessions.values()) {
    for (const p of s.pending.values()) p.settle({ result: 'rejected', reason: 'reset' })
    s.ws?.close?.()
  }
  sessions.clear()
  timelinesByRev.clear()
  wss?.close?.()
  wss = null
}
