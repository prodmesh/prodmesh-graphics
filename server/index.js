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
import { fileURLToPath } from 'node:url'
import path from 'node:path'

import { handleStream } from './streamHub.js'
import { registerTopics as registerCueLogTopics, since as cueLogSince } from './cueLog.js'
import { declare, report } from './health.js'
import { router as healthRouter } from './routes/health.js'
import { router as templatesRouter } from './routes/templates.js'
import { router as itemsRouter } from './routes/items.js'
import { router as cueRouter } from './routes/cue.js'
import { router as rundownRouter } from './routes/rundown.js'
import * as rundown from './rundown.js'
import * as nodeLink from './nodeLink.js'
import { build as buildAssignment } from './assignment.js'
import * as discovery from './discovery.js'
import * as cas from './cas.js'
import { open as openDb } from './db.js'
import { SCENE_READER_MAJOR } from './manifest.js'

const API_VERSION = 1
const PORT = Number(process.env.PORT ?? 8519)
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

  declare('server', 'Control server')
  report('server', 'ok')

  registerCueLogTopics()
  nodeLink.registerTopics()
  rundown.registerTopics()

  app.use(healthRouter)
  app.use(templatesRouter)
  app.use(itemsRouter)
  app.use(cueRouter)
  app.use(rundownRouter)

  app.get('/api/nodes', (_req, res) => res.json({ nodes: nodeLink.list() }))

  app.post('/api/nodes/:id/adopt', (req, res) => {
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
  app.put('/api/nodes/:id/outputs/:out', (req, res) => {
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
    openDb()
      .prepare(
        `INSERT INTO outputs (node_id, output_id, mode, layers, updated_at)
         VALUES (?, ?, ?, ?, ?)
         ON CONFLICT(node_id, output_id) DO UPDATE SET
           mode = excluded.mode, layers = excluded.layers, updated_at = excluded.updated_at`,
      )
      .run(
        req.params.id,
        req.params.out,
        String(req.body?.mode ?? '1920x1080'),
        JSON.stringify(layers),
        Date.now(),
      )
    // Push it immediately. A layer stack the operator just set must be live
    // before they try to take to it, not after the next reconnect.
    nodeLink.offerAssignment(req.params.id, buildAssignment({ nodeId: req.params.id }))
    res.json({ node: req.params.id, output: req.params.out, layers })
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
    express.raw({ type: '*/*', limit: '256mb' }),
    async (req, res) => {
      if (!req.body?.length) {
        return res.status(422).json({ error: 'validation_failed', label: 'That upload had no content.' })
      }
      const stored = await cas.put(req.body)
      const db = openDb()
      db.prepare(
        `INSERT OR IGNORE INTO assets (hash, size, kind, first_seen) VALUES (?, ?, ?, ?)`,
      ).run(stored.hash, stored.size, String(req.query.kind ?? 'blob'), Date.now())
      res.status(stored.deduped ? 200 : 201).json(stored)
    },
  )

  app.get('/api/assets', (_req, res) => {
    res.json({ assets: openDb().prepare('SELECT * FROM assets ORDER BY first_seen DESC').all() })
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
      milestone: 'M2',
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
if (process.argv[1] && import.meta.url === `file://${process.argv[1]}`) {
  const server = createApp().listen(PORT, () => {
    console.log(`ProdMesh Graphics control server on :${PORT} (M2)`)
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
