/**
 * Cache keys, shared between useQuery (pull) and stream (push).
 *
 * Kept in one file so two widgets asking for the same fact share one request
 * and one cache entry. That sharing is what makes "widgets fetch their own
 * state" affordable.
 */

export const keys = {
  version: () => '/api/version',
  health: () => '/api/health',

  templates: () => '/api/templates',
  template: (id: string) => `/api/templates/${id}`,
  templateRevisions: (id: string) => `/api/templates/${id}/revisions`,
  themes: () => '/api/themes',

  items: () => '/api/items',
  item: (id: string) => `/api/items/${id}`,
  /** Items whose label or typed values contain `q`, across every playlist. */
  itemSearch: (q: string) => `/api/items?q=${encodeURIComponent(q)}`,
  /** One playlist's own Items, arranged or not (ADR 0026). */
  playlistItems: (id: string) => `/api/items?playlist=${id}`,
  /** Every tag anybody has typed, with how many playlists carry it. */
  tags: () => '/api/tags',

  playlists: () => '/api/playlists',
  playlist: (id: string) => `/api/playlists/${id}`,
  /** A song read under one of its named orders (ADR 0027). Null is Main. */
  playlistUnder: (id: string, arrangement: string | null) =>
    arrangement ? `/api/playlists/${id}?arrangement=${arrangement}` : `/api/playlists/${id}`,
  arrangements: (playlistId: string) => `/api/playlists/${playlistId}/arrangements`,
  arrangement: (id: string) => `/api/arrangements/${id}`,
  rundowns: () => '/api/rundowns',
  rundown: (id: string) => `/api/rundowns/${id}`,
  assets: () => '/api/assets',
  assetSync: () => '/api/assets/sync',
  ingest: () => '/api/ingest',
  ingestJob: (id: string) => `/api/ingest/${id}`,

  nodes: () => '/api/nodes',
  /** The named places graphics go (ADR 0023). */
  screens: () => '/api/screens',
  users: () => '/api/users',
  stations: () => '/api/stations',
  node: (id: string) => `/api/nodes/${id}`,
  outputs: (nodeId: string) => `/api/nodes/${nodeId}/outputs`,
  /** A node's output HARDWARE — which device, which mode (ADR 0024). */
  outputConfig: (nodeId: string) => `/api/nodes/${nodeId}/output-config`,

  /** Gap recovery for the take log — the pull half of the ADR 0008 bridge. */
  cueLogSince: (showId: string, since: number) =>
    `/api/cuelog?show=${showId}&since=${since}`,
} as const

/** Snapshot topics. Every one of these is idempotent — newest value is truth. */
export const topics = {
  nodeHealth: (id: string) => `node:${id}:health`,
  nodeTelemetry: (id: string) => `node:${id}:telemetry`,
  nodeSync: (id: string) => `node:${id}:sync`,
  nodeStaging: (id: string) => `node:${id}:staging`,
  outputLayers: (id: string) => `output:${id}:layers`,
  itemReadiness: (id: string) => `item:${id}:readiness`,
  rundownCursor: (id: string) => `rundown:${id}:cursor`,
  previewStats: (id: string) => `preview:${id}:stats`,

  /**
   * The one event feed, and it is shaped as a snapshot: { head_seq, entries[] }.
   * See src/lib/stream.ts and ADR 0008 before adding anything event-shaped here.
   */
  cueLog: (showId: string) => `show:${showId}:cuelog`,
} as const
