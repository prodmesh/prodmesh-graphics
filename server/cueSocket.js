/**
 * The cue socket — Follow mode's path to the node (ADR 0017).
 *
 * # Why a socket, when `POST /api/cue` exists
 *
 * Follow takes on every keypress, and at chorus speed an operator presses
 * faster than a node acks. Each POST holds an HTTP/1.1 connection until its
 * ack, the browser allows six per origin, and the SSE stream already holds one
 * (no TLS, so no HTTP/2 — ADR 0008). The sixth keypress in a burst therefore
 * waited, invisibly, behind the first: the queueing ADR 0017 exists to remove.
 * One socket carries any number of cues in flight, each acked against the
 * `cue_id` the browser minted.
 *
 * # What it is NOT
 *
 * Not a second transport for everything — ADR 0008 rejected that, because a
 * socket "invites treating a cue like a value". It carries cues and their acks
 * and nothing else. Snapshots stay on SSE. And it is not a second
 * implementation: every cue goes through the same `dispatch` as the HTTP
 * endpoint and the same permission `check`, so the two paths cannot disagree
 * about dedupe, deadlines, superseding, fan-out or who may take. `POST
 * /api/cue` stays for Cue mode, control surfaces and scripts.
 *
 * # Protocol — JSON text frames
 *
 *   → { type: 'hello', station, bearer }       optional, first; the headers a
 *                                              browser socket cannot send
 *   → { type: 'cue', ...CueRequest }           any number, without waiting
 *   ← { type: 'ack', cue_id, http, ...payload } one per cue, in ack order
 *
 * `http` and `payload` are exactly what `POST /api/cue` would have answered,
 * so one client function interprets both.
 *
 * Spec: `docs/specs/http-api.md` §2.3.
 */

import { WebSocketServer } from 'ws'
import * as upgrade from './upgrade.js'
import { check, identifyTokens } from './auth.js'
import { context, cueScope, dispatch } from './routes/cue.js'

export const PATH = '/api/cue/socket'

/** Long enough to be free, short enough that a dead booth laptop is noticed. */
const PING_MS = 15_000

export function attach(server) {
  const wss = new WebSocketServer({ noServer: true })
  upgrade.route(server, PATH, wss)

  wss.on('connection', (ws) => {
    let tokens = { bearer: null, station: null }
    let alive = true
    ws.on('pong', () => { alive = true })

    ws.on('message', (raw) => {
      let msg
      try {
        msg = JSON.parse(String(raw))
      } catch {
        return reply(ws, { type: 'error', error: 'validation_failed', label: 'That was not a cue.' })
      }

      if (msg?.type === 'hello') {
        tokens = { bearer: msg.bearer ?? null, station: msg.station ?? null }
        return reply(ws, { type: 'ready' })
      }
      if (msg?.type !== 'cue') {
        return reply(ws, { type: 'error', error: 'validation_failed', label: 'Unknown message on the cue socket.' })
      }

      const { type: _type, ...body } = msg
      // Identity is resolved PER CUE, not once at hello: a session signed out
      // or a station un-trusted mid-service must stop being able to take on
      // the next keypress, not at the next reconnect.
      const who = identifyTokens(tokens)
      const refusal = check(who, 'graphics.operate', cueScope(body), PATH)
      if (refusal) {
        return reply(ws, { type: 'ack', cue_id: body.cue_id ?? null, http: refusal.status, ...refusal.payload })
      }

      // Not awaited before the next message is read — that is the point. The
      // node still sees these in arrival order: `dispatch` runs synchronously
      // up to `link.cue`, which assigns the link `seq`, so a later keypress
      // can never overtake an earlier one on its way to the node.
      dispatch({ surface: 'show_view', ...body }, context(who))
        .then(({ http, payload }) => reply(ws, { type: 'ack', cue_id: body.cue_id ?? null, http, ...payload }))
        .catch((e) => {
          console.error('cue socket: dispatch failed', e)
          reply(ws, {
            type: 'ack', cue_id: body.cue_id ?? null, http: 500, error: 'server_error',
            label: 'The control server could not finish that. Anything already on air is still on air.',
          })
        })
    })

    const ping = setInterval(() => {
      if (!alive) return ws.terminate()
      alive = false
      ws.ping()
    }, PING_MS)
    ping.unref?.()
    ws.on('close', () => clearInterval(ping))
    ws.on('error', () => clearInterval(ping))
  })
  return wss
}

function reply(ws, msg) {
  // The browser may have gone between the cue and its ack. The cue still
  // happened and is in the log; there is just nobody left to tell.
  if (ws.readyState === ws.OPEN) ws.send(JSON.stringify(msg))
}
