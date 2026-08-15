import { Router } from 'express'
import { publicHealth, snapshot } from '../health.js'
import { stats } from '../streamHub.js'

export const router = Router()

router.get('/api/health', (_req, res) => {
  res.json({ ok: true, milestone: 'M0', stream: stats(), components: snapshot() })
})

// Unauthenticated: safe for a screen on a wall. Fixed field list, no hosts,
// no ports, no error strings.
router.get('/api/health/public', (_req, res) => {
  res.json({ ok: true, components: publicHealth() })
})
