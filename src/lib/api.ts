/**
 * The write side. `useQuery` reads; this writes and reports failures as words.
 *
 * # Errors carry a `label`, and the label is what the operator sees
 *
 * The server answers every failure with `{ error, label, detail? }` where
 * `label` is a human sentence following `docs/UI_TEXT.md` — "Booth-1 is offline.
 * Anything already on air is still on air." rather than "Connection error."
 * Throwing away the label and rendering the code would undo that work at the
 * last step, so `ApiError` carries both and `message` IS the label.
 */

import { authHeaders, requestSignIn, sessionToken, stationToken } from './identity'

export class ApiError extends Error {
  readonly code: string
  readonly status: number
  readonly detail: unknown

  constructor(status: number, body: { error?: string; label?: string; detail?: unknown }) {
    super(body.label ?? 'The control server could not do that.')
    this.name = 'ApiError'
    this.status = status
    this.code = body.error ?? 'unknown'
    this.detail = body.detail
  }
}

async function send<T>(method: string, url: string, body?: unknown): Promise<T> {
  let res: Response
  try {
    res = await fetch(url, {
      method,
      headers: {
        ...authHeaders(),
        ...(body === undefined ? {} : { 'content-type': 'application/json' }),
      },
      body: body === undefined ? undefined : JSON.stringify(body),
    })
  } catch {
    // The fetch itself failed — the server is unreachable, not refusing. Those
    // are different situations for an operator and must not read the same.
    throw new ApiError(0, {
      error: 'unreachable',
      label: 'The control server is not answering. Anything already on air is still on air.',
    })
  }

  if (res.status === 204) return undefined as T
  const text = await res.text()
  const json = text ? (JSON.parse(text) as Record<string, unknown>) : {}
  if (!res.ok) throw refused(res.status, json)
  return json as T
}

/**
 * Turn a refusal into the error the caller throws — once, for every transport.
 *
 * A refusal for want of authority is the one failure the operator can fix
 * from here, so it opens the sign-in bar naming what was missing rather than
 * landing as one more sentence to read and dismiss. 401 and 403 both arrive
 * here and stay distinguishable: ApiError carries the status, and the bar says
 * "sign in" or "sign in as someone else" accordingly.
 */
function refused(status: number, json: Record<string, unknown>): ApiError {
  if (json.error === 'permission_required' && (status === 401 || status === 403)) {
    requestSignIn({ permission: String(json.permission ?? ''), label: String(json.label ?? '') })
  }
  return new ApiError(status, json)
}

/**
 * POST raw bytes — an upload, not a document.
 *
 * It exists so uploads share `send`'s failure handling rather than growing
 * their own. They did have their own, and once permissions came on it showed:
 * a refused upload read "the server answered 401" instead of the server's own
 * sentence, and nothing offered a way to sign in.
 */
export async function postBytes<T>(url: string, bytes: Uint8Array): Promise<T> {
  let res: Response
  try {
    res = await fetch(url, {
      method: 'POST',
      headers: { ...authHeaders(), 'content-type': 'application/octet-stream' },
      body: bytes as unknown as BodyInit,
    })
  } catch {
    throw new ApiError(0, {
      error: 'unreachable',
      label: 'The control server is not answering. Anything already on air is still on air.',
    })
  }
  const text = await res.text()
  const json = text ? (JSON.parse(text) as Record<string, unknown>) : {}
  if (!res.ok) throw refused(res.status, json)
  return json as T
}

export const get = <T>(url: string) => send<T>('GET', url)
export const post = <T>(url: string, body?: unknown) => send<T>('POST', url, body ?? {})
export const patch = <T>(url: string, body: unknown) => send<T>('PATCH', url, body)
export const put = <T>(url: string, body: unknown) => send<T>('PUT', url, body)
export const del = (url: string) => send<void>('DELETE', url)

/**
 * A cue id, generated **where the human acted**.
 *
 * This is the whole point of `cue_id`: it is minted at button-press and reused
 * on retry, so a double-click, a bounced GPI contact, or an HTTP retry after a
 * timeout all dedupe at the node. Generating a fresh one per attempt defeats
 * the mechanism entirely, which is why this is a separate call the caller makes
 * once, and never something `takeCue` does for itself.
 */
export function newCueId(): string {
  return crypto.randomUUID()
}

export type CueResult = {
  /**
   * `superseded` is a success: a newer take for the same layer replaced this
   * one before it fired (ADR 0017). It has no frame, because nothing went on.
   */
  result: 'applied' | 'deduped' | 'superseded'
  fired_at_frame?: number | null
  at_utc?: number
  seq: number
}

export type CueRequest = {
  cue_id: string
  node: string
  output?: string
  layer?: string
  // `take_also` fires the destinations a take deliberately skips (ADR 0018).
  op:
    | 'take'
    | 'take_also'
    | 'take_out'
    | 'arm'
    | 'update'
    | 'clear_layer'
    | 'clear_all'
    | 'preload'
    | 'timeline'
  item?: string
  data?: Record<string, unknown>
  /** For `op: 'timeline'` — the named timeline, and play (default) or stop. */
  timeline?: string
  action?: 'play' | 'stop'
  show?: string
}

/**
 * Fire a cue and wait for the node.
 *
 * This does not resolve until the node has confirmed or the deadline has
 * passed, because `POST /api/cue` is synchronous. Do not add an optimistic
 * update in front of it: a button that goes green before the graphic exists is
 * the exact failure the synchronous endpoint was built to prevent.
 */
export const takeCue = (req: CueRequest) =>
  post<CueResult>('/api/cue', { surface: 'show_view', ...req })

// ---------------------------------------------------------------------------
// The cue socket — Follow mode (ADR 0017)
//
// Follow takes on every keypress, faster than a node acks. Over HTTP/1.1 each
// un-acked POST holds one of the six connections the browser allows per
// origin, and the SSE stream holds another, so a burst queued invisibly behind
// its own first keypress. One socket carries any number of cues in flight.
//
// The socket is an optimisation, never a dependency. Anything it cannot
// deliver — not open, dropped mid-cue, silent too long — goes by `POST
// /api/cue` with the SAME cue_id, which is safe precisely because the node
// dedupes on it: a cue that did fire answers `deduped`, one that did not
// fires now. Nothing is lost and nothing fires twice.

type InFlight = {
  body: CueRequest & { surface: string }
  resolve: (r: CueResult) => void
  reject: (e: unknown) => void
  sent: boolean
  timer: ReturnType<typeof setTimeout> | null
}

/**
 * How long to wait for an ack before asking again over HTTP. The server's own
 * wait is the deadline plus 250 ms (half a second by default), so this only
 * fires when the socket itself has gone quiet.
 */
const SOCKET_ACK_GIVE_UP_MS = 5000
/** After a failed connect, Follow uses HTTP for a while rather than retrying per keypress. */
const SOCKET_RETRY_MS = 2000

let cueSock: WebSocket | null = null
let cueSockHello: string | null = null
let cueSockRetryAt = 0
const inFlight = new Map<string, InFlight>()

function cueSocketUrl() {
  const { protocol, host } = window.location
  return `${protocol === 'https:' ? 'wss' : 'ws'}://${host}/api/cue/socket`
}

/** By HTTP instead, keeping the cue_id. */
function viaHttp(f: InFlight) {
  if (f.timer) clearTimeout(f.timer)
  inFlight.delete(f.body.cue_id)
  post<CueResult>('/api/cue', f.body).then(f.resolve, f.reject)
}

function transmit(ws: WebSocket, f: InFlight) {
  // Identity can change between keypresses — somebody signs in mid-service.
  // The server resolves it per cue from the latest hello, so say it again
  // whenever it has changed rather than reconnecting.
  const bearer = sessionToken()
  const station = stationToken()
  const key = `${bearer ?? ''}|${station ?? ''}`
  if (key !== cueSockHello) {
    ws.send(JSON.stringify({ type: 'hello', bearer, station }))
    cueSockHello = key
  }
  ws.send(JSON.stringify({ type: 'cue', ...f.body }))
  f.sent = true
  f.timer = setTimeout(() => viaHttp(f), SOCKET_ACK_GIVE_UP_MS)
}

function cueSocket(): WebSocket | null {
  if (typeof WebSocket === 'undefined') return null
  if (cueSock && (cueSock.readyState === WebSocket.OPEN || cueSock.readyState === WebSocket.CONNECTING)) {
    return cueSock
  }
  if (Date.now() < cueSockRetryAt) return null

  const ws = new WebSocket(cueSocketUrl())
  cueSock = ws
  cueSockHello = null
  ws.onopen = () => {
    for (const f of inFlight.values()) if (!f.sent) transmit(ws, f)
  }
  ws.onmessage = (e) => {
    let msg: Record<string, unknown>
    try {
      msg = JSON.parse(String(e.data))
    } catch {
      return
    }
    if (msg.type !== 'ack') return
    const f = inFlight.get(String(msg.cue_id))
    if (!f) return
    if (f.timer) clearTimeout(f.timer)
    inFlight.delete(f.body.cue_id)
    const { type: _t, http, ...payload } = msg
    const status = Number(http)
    if (status >= 200 && status < 300) f.resolve(payload as CueResult)
    else f.reject(refused(status, payload))
  }
  const lost = () => {
    if (cueSock !== ws) return
    cueSock = null
    cueSockRetryAt = Date.now() + SOCKET_RETRY_MS
    for (const f of inFlight.values()) viaHttp(f)
  }
  ws.onclose = lost
  ws.onerror = lost
  return ws
}

/**
 * Fire a Follow-mode cue without holding a connection per keypress.
 *
 * Resolves exactly what `takeCue` would — the same payload, the same
 * `ApiError` on refusal — because the server answers the socket with the
 * status and body `POST /api/cue` would have. Still no optimism: this resolves
 * on the node's ack, and the caller sets nothing on air from it.
 */
export function followCue(req: CueRequest): Promise<CueResult> {
  const body = { surface: 'show_view', ...req }
  const ws = cueSocket()
  if (!ws) return post<CueResult>('/api/cue', body)
  return new Promise<CueResult>((resolve, reject) => {
    const f: InFlight = { body, resolve, reject, sent: false, timer: null }
    inFlight.set(req.cue_id, f)
    if (ws.readyState === WebSocket.OPEN) transmit(ws, f)
  })
}

/** Test seam: forget the cue socket and anything in flight. */
export function _reset() {
  for (const f of inFlight.values()) if (f.timer) clearTimeout(f.timer)
  inFlight.clear()
  cueSock = null
  cueSockHello = null
  cueSockRetryAt = 0
}
