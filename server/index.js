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
 * M0: health endpoints and the topic stream. Everything else is M1.
 */

import express from 'express'
import { fileURLToPath } from 'node:url'
import path from 'node:path'

import { handleStream } from './streamHub.js'
import { registerTopics as registerCueLogTopics } from './cueLog.js'
import { declare, report } from './health.js'
import { router as healthRouter } from './routes/health.js'

const PORT = Number(process.env.PORT ?? 8519)
const HERE = path.dirname(fileURLToPath(import.meta.url))

export function createApp() {
  const app = express()
  app.disable('x-powered-by')
  app.use(express.json({ limit: '4mb' }))

  declare('server', 'Control server')
  report('server', 'ok')

  registerCueLogTopics()

  app.use(healthRouter)
  app.get('/api/stream', handleStream)

  app.get('/api/version', (_req, res) => {
    res.json({ api: 1, server: '0.1.0', proto_ver: 1, scene_reader_major: 1, milestone: 'M0' })
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
    console.log(`ProdMesh Graphics control server on :${PORT} (M0 scaffold)`)
  })
}
