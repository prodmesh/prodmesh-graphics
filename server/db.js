/**
 * SQLite (WAL) — the server's store of record for content and assignment.
 *
 * NOT the store of record for what is on air. The node is authoritative there,
 * and the server adopts its StateReport rather than overwriting it (ADR 0009).
 * Nothing in this schema should tempt anyone to believe otherwise: there is no
 * `on_air` column, and there will not be one.
 *
 * SceneDocs are NOT stored here. They are opaque CAS blobs; this holds the
 * compiler-emitted manifest sidecar, which is all the server ever validates
 * (ADR 0004).
 *
 * M0: schema sketched, not created. Lands in M1 with the real routes.
 */

export const SCHEMA_VERSION = 0

export const PLANNED_TABLES = [
  'templates',          // id, name, created_at
  'template_revisions', // template_rev (blake3, PK), template_id, manifest JSON, published_at
  'items',              // id, template_id, template_rev (PINNED), data JSON, destination JSON
  'playlists', 'playlist_items',
  'rundowns', 'rundown_playlists',
  'nodes',              // node_id, name, adopted, last_seen, capabilities JSON
  'outputs',            // node_id, output_id, config JSON (mode, colour, key_mode, layers)
  'assets',             // hash (PK), size, kind, colour tags, first_seen
  'assignments',        // node_id, gen, payload, digest, acked_at
  'users', 'groups', 'group_permissions', 'stations', 'sessions', 'audit_log',
]

export function open(_path) {
  throw new Error('server/db.js is an M0 stub — see docs/ROADMAP.md M1')
}
