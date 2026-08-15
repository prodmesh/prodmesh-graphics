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
  state: { outputs: { id: string }[] } | null
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
