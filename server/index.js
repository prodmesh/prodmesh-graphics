/**
 * ProdMesh Graphics control server.
 *
 * Express 5, plain ESM, no build step — ProdMesh-Control's shape, deliberately
 * (ADR 0001). Port 8519, in the family's 85xx band.
 *
 * What this server is authoritative for: content and assignment.
 * What it is NOT authoritative for: what is on air. That is the node's, and on
 * reconnect this server READS its StateReport and adopts it. A server restart
 * must not change a pixel (ADR 0009).
 *
 * Threat model, inherited: LAN appliance on a closed show network, no TLS,
 * binds all interfaces. No TLS means no HTTP/2 means six connections per origin
 * in the browser — which is why live data rides ONE SSE connection multiplexed
 * by topic, and why preview gets its own port rather than a connection each.
 *
 * M1e: content (templates, revisions, items), the CAS origin, the topic
 * stream, and the cue log's gap endpoint.
 */

import express from 'express'
import { pathToFileURL } from 'node:url'
import { fileURLToPath } from 'node:url'
import path from 'node:path'

import { handleStream } from './streamHub.js'
import { registerTopics as registerCueLogTopics, since as cueLogSince } from './cueLog.js'
import { declare, report } from './health.js'
import { router as healthRouter } from './routes/health.js'
import { router as templatesRouter } from './routes/templates.js'
import { router as itemsRouter } from './routes/items.js'
import { router as themesRouter } from './routes/themes.js'
import { router as screensRouter } from './routes/screens.js'
import { router as nodeOutputsRouter } from './routes/nodeOutputs.js'
import { router as cueRouter } from './routes/cue.js'
import { router as rundownRouter } from './routes/rundown.js'
import { router as authRouter } from './routes/auth.js'
import { attach as attachIdentity, requirePermission } from './auth.js'
import * as authStore from './authStore.js'
import * as rundown from './rundown.js'
import * as nodeLink from './nodeLink.js'
import { build as buildAssignment } from './assignment.js'
import * as discovery from './discovery.js'
import * as ingest from './ingest.js'
import * as cas from './cas.js'
import { open as openDb } from './db.js'
import { SCENE_READER_MAJOR } from './manifest.js'

const API_VERSION = 1

/**
 * Which milestone this build is from, shown beside the wordmark.
 *
 * Hard-coded and stated ONCE, because it was hard-coded in two places and read
 * "M2" for four milestones — a version badge that lies is worse than no badge,
 * since it is the first thing anyone quotes when reporting a problem.
 */
const MILESTONE = 'M6'

/**
 * The listen port: `--port <n>` beats `PMG_PORT` beats `PORT` beats 8519.
 *
 * The explicit flag exists because dev harnesses (Claude Code's preview, some
 * IDE runners) inject a generic `PORT` into everything they spawn — which sent
 * this server racing Vite for the same socket, and the symptom was a UI whose
 * every API call 500'd while both processes claimed to be running fine.
 * `npm run dev` pins the port with the flag so no environment can move it.
 */
function listenPort() {
  const flag = process.argv.indexOf('--port')
  if (flag !== -1 && process.argv[flag + 1]) return Number(process.argv[flag + 1])
  return Number(process.env.PMG_PORT ?? process.env.PORT ?? 8519)
}
const PORT = listenPort()
const HERE = path.dirname(fileURLToPath(import.meta.url))

export function createApp() {
  const app = express()
  app.disable('x-powered-by')
  // 4 MB covers a compiled SceneDoc with room to spare. It deliberately does
  // NOT cover media: assets go to the CAS over their own route, because a 4 GB
  // video must never travel a path that a take also has to wait behind.
  app.use(express.json({ limit: '4mb' }))
  app.use((_req, res, next) => {
    res.setHeader('X-PMG-Api', String(API_VERSION))
    next()
  })

  openDb()
  cas.init()
  authStore.sweep()

  // Who is asking, on every request. Never throws; an unreadable token is
  // simply nobody.
  app.use(attachIdentity)

  declare('server', 'Control server')
  report('server', 'ok')

  registerCueLogTopics()
  nodeLink.registerTopics()
  rundown.registerTopics()

  app.use(healthRouter)
  app.use(authRouter)
  app.use(templatesRouter)
  app.use(itemsRouter)
  app.use(themesRouter)
  app.use(screensRouter)
  app.use(nodeOutputsRouter)
  app.use(cueRouter)
  app.use(rundownRouter)

  app.get('/api/nodes', (_req, res) => res.json({ nodes: nodeLink.list() }))

  app.post('/api/nodes/:id/adopt', requirePermission('graphics.nodes.configure'), (req, res) => {
    const adopted = req.body?.adopted !== false
    const n = nodeLink.setAdopted(req.params.id, adopted)
    if (!n) return res.status(404).json({ error: 'not_found', label: 'No node with that id has connected.' })
    res.json(n)
  })

  app.get('/api/nodes/:id/outputs', (req, res) => {
    const rows = openDb()
      .prepare('SELECT * FROM outputs WHERE node_id = ? ORDER BY output_id')
      .all(req.params.id)
    res.json({
      outputs: rows.map((o) => ({ ...o, layers: JSON.parse(o.layers) })),
      // What the node is actually running right now, which is the node's
      // answer and not ours.
      live: nodeLink.get(req.params.id)?.state?.outputs ?? [],
    })
  })

  /**
   * Configure an output's layer stack.
   *
   * The array is ORDERED, bottom first, and that order is the compositing
   * order. Reordering it changes what covers what; renaming a layer is a
   * different operation entirely, because items address layers by name and a
   * rename would orphan them silently.
   */
  app.put('/api/nodes/:id/outputs/:out', requirePermission('graphics.nodes.configure'), (req, res) => {
    const layers = req.body?.layers
    if (!Array.isArray(layers) || layers.some((l) => typeof l !== 'string' || !l.trim())) {
      return res.status(422).json({
        error: 'validation_failed',
        label: 'An output needs an ordered list of layer names.',
      })
    }
    if (new Set(layers).size !== layers.length) {
      // Two layers with one name make "take to Lower Third" ambiguous, and the
      // node would resolve it to whichever came first.
      return res.status(422).json({
        error: 'validation_failed',
        label: 'Two layers on the same output can’t share a name.',
      })
    }
    const MATRICES = new Set(['bt601', 'bt709', 'bt2020'])
    const RANGES = new Set(['narrow', 'full'])
    const KEY_MODES = new Set(['fill', 'key', 'fill_key', 'embedded'])
    const c = req.body?.color ?? {}
    const matrix = c.matrix ?? 'bt709'
    const range = c.range ?? 'narrow'
    const keyRange = c.key_range ?? 'narrow'
    const keyMode = c.key_mode ?? 'fill'
    // Refused by name rather than silently coerced to a default: a typo that
    // quietly becomes 709 narrow is a colour bug nobody can find later.
    if (!MATRICES.has(matrix) || !RANGES.has(range) || !RANGES.has(keyRange) || !KEY_MODES.has(keyMode)) {
      return res.status(422).json({
        error: 'validation_failed',
        label: 'That is not a colour setting this server knows.',
      })
    }

    openDb()
      .prepare(
        `INSERT INTO outputs (node_id, output_id, mode, layers, updated_at, matrix, range, key_range, key_mode)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(node_id, output_id) DO UPDATE SET
           mode = excluded.mode, layers = excluded.layers, updated_at = excluded.updated_at,
           matrix = excluded.matrix, range = excluded.range,
           key_range = excluded.key_range, key_mode = excluded.key_mode`,
      )
      .run(
        req.params.id,
        req.params.out,
        String(req.body?.mode ?? '1920x1080'),
        JSON.stringify(layers),
        Date.now(),
        matrix,
        range,
        keyRange,
        keyMode,
      )
    // Push it immediately. A layer stack the operator just set must be live
    // before they try to take to it, not after the next reconnect.
    nodeLink.offerAssignment(req.params.id, buildAssignment({ nodeId: req.params.id }))
    res.json({ node: req.params.id, output: req.params.out, layers })
  })

  /**
   * Put a test pattern up, or clear it with `{ pattern: null }`.
   *
   * A configuration you cannot PROVE is one you are trusting. These are the
   * instruments (`docs/specs/color-and-key.md` §7), and they sit above the
   * layer stack so they can be used without clearing the show off air.
   */
  app.post('/api/nodes/:id/outputs/:out/test-pattern',
    requirePermission('graphics.nodes.configure'),
    async (req, res) => {
      const r = await nodeLink.cue(req.params.id, {
        cue_id: req.body?.cue_id ?? `pattern-${Date.now()}`,
        op: { TestPattern: { output: req.params.out, pattern: req.body?.pattern ?? null } },
      })
      if (r.result === 'applied' || r.result === 'deduped') return res.json(r)
      if (r.result === 'unknown_node') {
        return res.status(404).json({ error: 'no_such_node', label: 'No node with that id has connected.' })
      }
      if (r.result === 'offline') {
        return res.status(503).json({ error: 'node_offline', label: `${req.params.id} is offline.` })
      }
      res.status(409).json({
        error: r.reason ?? 'rejected',
        label: r.reason === 'Unsupported'
          ? 'That node does not have a test pattern by that name.'
          : 'The node refused that.',
      })
    })
  app.get('/api/nodes/:id', (req, res) => {
    const n = nodeLink.get(req.params.id)
    if (!n) return res.status(404).json({ error: 'not_found', label: 'No node with that id has connected.' })
    res.json(n)
  })
  app.get('/api/stream', handleStream)
  app.get('/cas/:hash', cas.handleGet)

  // Asset upload. Raw body, not multipart: the client already knows the bytes
  // and a multipart wrapper would only add a parser between them and the hash.
  //
  // 256 MB covers a font, an image, and a short clip. Large media gets a
  // resumable path in M3 — the limit is here so an accident fails fast rather
  // than filling a booth machine's disk.
  app.post(
    '/api/assets',
    requirePermission('graphics.assets.manage'),
    express.raw({ type: '*/*', limit: '256mb' }),
    async (req, res) => {
      if (!req.body?.length) {
        return res.status(422).json({ error: 'validation_failed', label: 'That upload had no content.' })
      }
      const stored = await cas.put(req.body)
      const db = openDb()
      const name = String(req.query.name ?? '').trim() || null
      db.prepare(
        `INSERT OR IGNORE INTO assets (hash, size, kind, first_seen, name) VALUES (?, ?, ?, ?, ?)`,
      ).run(stored.hash, stored.size, String(req.query.kind ?? 'blob'), Date.now(), name)
      // The same bytes uploaded again are the same asset, and its name is not
      // overwritten — somebody may have renamed it since. But a row that never
      // got one takes this upload's, so a library that predates names fills in
      // as it is used rather than staying a wall of hashes.
      if (name) db.prepare(`UPDATE assets SET name = ? WHERE hash = ? AND name IS NULL`).run(name, stored.hash)
      res.status(stored.deduped ? 200 : 201).json(stored)
    },
  )

  /**
   * Ingest media: probe it, and convert it to an all-intra intermediate if it
   * cannot seek frame by frame.
   *
   * Returns a JOB, not a result. A service-length clip takes minutes to
   * convert, and holding an HTTP request open for that is a request that will
   * be dropped by something in the middle.
   */
  app.post(
    '/api/assets/ingest',
    requirePermission('graphics.assets.manage'),
    express.raw({ type: '*/*', limit: '8gb' }),
    async (req, res) => {
      if (!req.body?.length) {
        return res.status(422).json({ error: 'validation_failed', label: 'That upload had no content.' })
      }
      const job = await ingest.start({
        bytes: req.body,
        filename: String(req.query.filename ?? 'upload'),
      })
      res.status(202).json(job)
    },
  )

  /**
   * Rename an asset.
   *
   * The hash stays the identity — nothing about a render changes here. This is
   * the label a designer reads in a picker, and `4a3b0d7b` is not one.
   */
  app.patch('/api/assets/:hash', requirePermission('graphics.assets.manage'), (req, res) => {
    const name = String(req.body?.name ?? '').trim()
    if (!name) {
      return res.status(422).json({ error: 'validation_failed', label: 'An asset needs a name.' })
    }
    const info = openDb().prepare('UPDATE assets SET name = ? WHERE hash = ?').run(name, req.params.hash)
    if (info.changes === 0) {
      return res.status(404).json({ error: 'not_found', label: 'This server does not hold that asset.' })
    }
    res.json({ hash: req.params.hash, name })
  })

  app.get('/api/ingest', (_req, res) => res.json({ jobs: ingest.list() }))
  app.get('/api/ingest/:id', (req, res) => {
    const j = ingest.get(req.params.id)
    if (!j) return res.status(404).json({ error: 'not_found', label: 'There is no ingest job with that id.' })
    res.json(j)
  })

  /**
   * The Media Browser's one answer: every asset, which templates use it, and
   * each adopted node's possession of it — derived from what nodes already
   * report, no new protocol. `used_by` reads head revisions only: an old
   * revision's assets matter to a node that is pinned to it, and that shows in
   * the node's own required list rather than here.
   */
  app.get('/api/assets/sync', (_req, res) => {
    const db = openDb()
    const usedBy = {}
    const heads = db.prepare(
      `SELECT t.name AS name, r.manifest AS manifest
         FROM templates t JOIN template_revisions r ON r.template_rev = t.head_rev`,
    ).all()
    for (const h of heads) {
      let manifest
      try { manifest = JSON.parse(h.manifest) } catch { continue }
      for (const hash of manifest.assets ?? []) {
        ;(usedBy[hash] ??= []).push(h.name)
      }
    }
    // Items reference media through their DATA (a field-driven clip or
    // picture), not through a template manifest — both are "used by", and
    // telling a user their on-air clip is "not used" is the kind of lie that
    // gets an asset deleted on a Thursday.
    const items = db.prepare('SELECT label, data FROM items').all()
    for (const it of items) {
      let data
      try { data = JSON.parse(it.data ?? '{}') } catch { continue }
      for (const v of Object.values(data)) {
        if (typeof v === 'string' && /^blake3:[0-9a-f]{64}$/.test(v)) {
          ;(usedBy[v] ??= []).push(`Item “${it.label}”`)
        }
      }
    }
    res.json({
      assets: db
        .prepare('SELECT * FROM assets ORDER BY first_seen DESC')
        .all()
        .map((a) => ({ ...a, media: a.media ? JSON.parse(a.media) : null })),
      used_by: usedBy,
      nodes: nodeLink.assetSync(),
    })
  })

  app.get('/api/assets', (_req, res) => {
    res.json({
      assets: openDb()
        .prepare('SELECT * FROM assets ORDER BY first_seen DESC')
        .all()
        .map((a) => ({ ...a, media: a.media ? JSON.parse(a.media) : null })),
    })
  })
  app.head('/cas/:hash', cas.handleGet)

  // The pull half of the ADR 0008 bridge: a client that missed frames on the
  // conflating topic fetches the gap here rather than being told to resync.
  app.get('/api/cuelog', (req, res) => {
    res.json(cueLogSince(
      String(req.query.show ?? 'default'),
      Number(req.query.since ?? 0),
      Math.min(Number(req.query.limit ?? 500), 500),
    ))
  })

  app.get('/api/version', (_req, res) => {
    res.json({
      api: API_VERSION,
      server: '0.1.0',
      proto_ver: 1,
      scene_reader_major: SCENE_READER_MAJOR,
      milestone: MILESTONE,
    })
  })

  // Every failure leaves here as JSON with a human `label`, including ones
  // nobody anticipated — including a rejected promise from an `async` route.
  // Express 5 forwards those here; Express 4 did not, which is what oxlint's
  // `no-async-endpoint-handlers` guards against. That rule is off in
  // .oxlintrc.json (JSON cannot hold the reason, so it lives here): leaving it
  // on would wrap every async route in a `.catch(next)` that implies a hazard
  // this server does not have. Verified against Express 5 rather than assumed.
  // Express's default handler returns an HTML stack trace,
  // which a Show view can only render as "something went wrong" — and on a
  // closed booth network the stack is leaked to no useful audience anyway.
  // UI_TEXT.md's rule applies even to the unexpected: name the thing.
  app.use((err, req, res, next) => {
    if (res.headersSent) return next(err)
    console.error(`${req.method} ${req.originalUrl}`, err)
    res.status(500).json({
      error: 'server_error',
      label: 'The control server could not finish that. Anything already on air is still on air.',
      detail: err?.message ?? String(err),
    })
  })

  if (process.env.NODE_ENV === 'production') {
    const dist = path.join(HERE, '..', 'dist')
    app.use(express.static(dist))
    // Client-side routing: anything not under /api or /cas is the SPA.
    app.get(/^\/(?!api|cas|link).*/, (_req, res) => res.sendFile(path.join(dist, 'index.html')))
  }

  return app
}

// Only listen when run directly, so tests can import createApp() freely.
//
// Compared as a URL, not as a string with `file://` glued on the front. A path
// containing a SPACE encodes as %20 in `import.meta.url` and does not in the
// concatenation, so the two never matched — and the server started, listened to
// nothing, and exited 0 with no output. It cannot happen in this repo, where
// the path has no spaces; it happens the moment the thing is installed as
// "ProdMesh Director.app", which is to say in production and nowhere else.
if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const server = createApp().listen(PORT, () => {
    console.log(`ProdMesh Graphics control server on :${PORT} (${MILESTONE})`)
  })
  // PMG Link shares the HTTP listener: it is a WebSocket upgrade on /link, so
  // it needs the server object rather than the app. Assets and preview stay
  // elsewhere on purpose (pmg-link.md §0).
  nodeLink.attach(server)

  // Advertised so a node with no stored pairing can find us. Manual entry
  // always works and is the path that must never depend on this.
  discovery.advertise({ port: PORT, serverId: nodeLink.serverId() })
  process.on('SIGINT', shutdown)
  process.on('SIGTERM', shutdown)
}

/** Withdraw the mDNS advertisement rather than leaving a ghost service behind. */
function shutdown() {
  discovery.stop()
  process.exit(0)
}
