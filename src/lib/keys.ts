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

  items: () => '/api/items',
  item: (id: string) => `/api/items/${id}`,

  playlists: () => '/api/playlists',
  playlist: (id: string) => `/api/playlists/${id}`,
  rundowns: () => '/api/rundowns',
  rundown: (id: string) => `/api/rundowns/${id}`,
  assets: () => '/api/assets',

  nodes: () => '/api/nodes',
  node: (id: string) => `/api/nodes/${id}`,
  outputs: (nodeId: string) => `/api/nodes/${nodeId}/outputs`,

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
