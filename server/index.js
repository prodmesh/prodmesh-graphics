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

  app.use(healthRouter)
  app.use(templatesRouter)
  app.use(itemsRouter)
  app.get('/api/stream', handleStream)
  app.get('/cas/:hash', cas.handleGet)
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
      milestone: 'M1',
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
  createApp().listen(PORT, () => {
    console.log(`ProdMesh Graphics control server on :${PORT} (M1)`)
  })
}
