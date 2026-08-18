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
      headers: body === undefined ? {} : { 'content-type': 'application/json' },
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
  if (!res.ok) throw new ApiError(res.status, json)
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
  result: 'applied' | 'deduped'
  fired_at_frame: number | null
  at_utc: number
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
  item?: string
  data?: Record<string, unknown>
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
