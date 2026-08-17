/**
 * Shapes the server sends. Deliberately structural: the browser mirrors what
 * the API returns and does not model the scene format, which is Rust's.
 */

export type Item = {
  id: string
  take_id: string | null
  label: string
  template_id: string
  template_rev: string
  data: Record<string, string | number | boolean>
  destination: { node?: string; output?: string; layer?: string }
  updated_at: number
}

export type Occupant = {
  item_id: string
  item_label?: string
  template_rev: string
  /** `in` | `hold` | `out` */
  phase: string
  timeline: string | null
  t_ms: number
  /** Something on air could not be drawn as authored. Saying so is the point. */
  degraded: boolean
}

export type LayersSnapshot = {
  output: string
  node: string
  fps?: number
  late_frames?: number
  layers: { name: string; occupant: Occupant | null }[]
}

export type NodeSummary = {
  node_id: string
  name: string
  reachable: boolean
  version: string | null
  assignment_gen: number | null
  last_seen: number
  /** The node's own report of its outputs and their layers. Never our idea of them. */
  state: { outputs: OutputState[] } | null
  /**
   * Where to fetch this node's MJPEG preview.
   *
   * Points at the NODE, not at the control server: preview is a separate
   * connection on a separate port so that a stalled or bandwidth-hungry viewer
   * cannot delay a cue (ADR 0007).
   */
  preview_url?: string | null
}

export type SyncState = { present: boolean; locked: boolean; format: string }

export type OutputState = {
  id: string
  /**
   * Reference/genlock as the hardware reports it.
   *
   * `undefined` means the backend has no such concept (NDI, headless) — which
   * is NOT the same as `present: false`, and showing them alike would tell an
   * operator their card had lost reference when it never had one.
   */
  sync?: SyncState | null
  fps?: number
  late_frames?: number
  layers: { name: string; occupant: Occupant | null }[]
}

export type CueLogEntry = {
  seq: number
  at_utc: number
  fired_at_frame: number | null
  cue_id: string
  node: string | null
  output: string | null
  layer: string | null
  op: string
  item: string | null
  item_label: string | null
  surface: string
  user: string | null
  station: string | null
  result: string
}

/** `{ head_seq, entries[] }` — a log tail as a snapshot, never a bare event. */
export type CueLogSnapshot = { head_seq: number; entries: CueLogEntry[] }

/**
 * What every node together says about one Item (ADR 0018).
 *
 * `ready` is **at least one destination ready**, not all of them — which is why
 * `partial` has to exist beside it. Without that second flag the operator's
 * only two states are "take it" and "cannot take it", and the interesting one
 * — takeable, but it will not reach the wall — has nowhere to live.
 */
export type Readiness = {
  item: string
  ready: boolean
  partial: boolean
  missing: string[]
  unreadable: string | null
  destinations: DestinationReadiness[]
}

export type DestinationReadiness = {
  node: string
  output: string
  layer: string
  ready: boolean
  missing: string[]
  unreadable: string | null
}
