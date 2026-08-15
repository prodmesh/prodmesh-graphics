/**
 * SQLite (WAL) — the server's store of record for content and assignment.
 *
 * NOT the store of record for what is on air. The node is authoritative there,
 * and the server adopts its StateReport rather than overwriting it (ADR 0009).
 * Nothing in this schema should tempt anyone to believe otherwise: **there is
 * no `on_air` column, and there will not be one.** If you find yourself wanting
 * to add one, what you actually want is to read `output:<id>:layers`, which the
 * node publishes.
 *
 * SceneDocs are NOT stored here. They are opaque CAS blobs; this holds the
 * compiler-emitted manifest sidecar, which is all the server ever validates
 * (ADR 0004).
 */

import Database from 'better-sqlite3'
import fs from 'node:fs'
import path from 'node:path'

export const SCHEMA_VERSION = 5

let db = null

/**
 * Migrations, applied in order, each exactly once. `user_version` is the
 * counter — the same mechanism ProdMesh-Control uses, for the same reason: it
 * is a single integer in the database file itself, so it cannot get out of step
 * with the file the way a separate table can if a restore goes sideways.
 */
const MIGRATIONS = [
  // 1 — the M1 content model.
  (d) => {
    d.exec(`
      CREATE TABLE templates (
        id          TEXT PRIMARY KEY,
        name        TEXT NOT NULL,
        created_at  INTEGER NOT NULL,
        updated_at  INTEGER NOT NULL,
        -- The newest published revision. NULL means authored but never
        -- published, which is a normal state: Design saves drafts.
        --
        -- Deliberately NOT a foreign key. Pairing it with
        -- template_revisions.template_id would make a reference CYCLE, and
        -- SQLite cannot break a cycle without deferred constraints — so
        -- deleting a template becomes impossible in any single statement order.
        -- The integrity that actually matters (no orphan revisions) is the
        -- other direction's cascade, and head_rev has exactly one writer: the
        -- publish transaction, which just inserted the row it points at.
        head_rev    TEXT
      );

      -- Immutable. A revision is named by the hash of its own bytes, so
      -- "updating" one is a contradiction in terms (ADR 0006).
      CREATE TABLE template_revisions (
        template_rev  TEXT PRIMARY KEY,
        template_id   TEXT NOT NULL REFERENCES templates(id) ON DELETE CASCADE,
        name          TEXT NOT NULL,
        min_reader    INTEGER NOT NULL,
        -- The compiler's manifest, verbatim. The server validates THIS and
        -- never the SceneDoc.
        manifest      TEXT NOT NULL,
        published_at  INTEGER NOT NULL,
        published_by  TEXT
      );
      CREATE INDEX template_revisions_by_template
        ON template_revisions(template_id, published_at DESC);

      CREATE TABLE items (
        id           TEXT PRIMARY KEY,
        -- The broadcast-facing alias (ADR 0015). Stable, integrator-visible,
        -- never shown in our own UI.
        take_id      TEXT UNIQUE,
        label        TEXT NOT NULL,
        template_id  TEXT NOT NULL REFERENCES templates(id),
        -- PINNED, not a pointer to head. Publishing a new revision must never
        -- silently change what an existing Item renders — that is how a
        -- rundown quietly breaks between Thursday and Sunday.
        template_rev TEXT NOT NULL REFERENCES template_revisions(template_rev),
        data         TEXT NOT NULL,
        destination  TEXT NOT NULL,
        created_at   INTEGER NOT NULL,
        updated_at   INTEGER NOT NULL
      );
      CREATE INDEX items_by_template ON items(template_id);

      CREATE TABLE assets (
        hash        TEXT PRIMARY KEY,
        size        INTEGER NOT NULL,
        kind        TEXT NOT NULL,
        first_seen  INTEGER NOT NULL
      );

      CREATE TABLE nodes (
        node_id       TEXT PRIMARY KEY,
        name          TEXT NOT NULL,
        adopted       INTEGER NOT NULL DEFAULT 0,
        last_seen     INTEGER,
        capabilities  TEXT
      );

      -- What the server has told a node to be able to play. Not what IS
      -- playing (see the header).
      CREATE TABLE assignments (
        node_id   TEXT PRIMARY KEY REFERENCES nodes(node_id) ON DELETE CASCADE,
        gen       INTEGER NOT NULL,
        payload   TEXT NOT NULL,
        digest    TEXT NOT NULL,
        sent_at   INTEGER NOT NULL,
        acked_at  INTEGER
      );
    `)
  },

  // 2 — keep the JSON mirror alongside each revision.
  //
  // A published revision is canonical CBOR, which Design cannot read and
  // deliberately never learns to. Without the mirror the only way to reopen a
  // template is to still have the file that made it, which makes Design a
  // one-way door and loses the template the first time a laptop is reimaged.
  //
  // The server still does not parse it: it is another opaque CAS blob, stored
  // by address, and this column is just the pointer.
  (d) => {
    d.exec(`ALTER TABLE template_revisions ADD COLUMN mirror_hash TEXT`)
  },

  // 3 — outputs and their layer stacks.
  //
  // Layers are ORDERED and addressed BY NAME. The order is the compositing
  // order, bottom first, and it is stored as a JSON array rather than a row per
  // layer with an index column — because an index column invites code that
  // addresses a layer by its position, which silently re-points at a different
  // graphic the moment someone reorders the stack.
  (d) => {
    d.exec(`
      CREATE TABLE outputs (
        node_id    TEXT NOT NULL,
        output_id  TEXT NOT NULL,
        mode       TEXT NOT NULL,
        layers     TEXT NOT NULL,
        updated_at INTEGER NOT NULL,
        PRIMARY KEY (node_id, output_id)
      );
    `)
  },

  // 4 — playlists, the rundown, and the cursor.
  //
  // `position` is a dense integer renumbered on every reorder. A rundown is
  // tens of entries, so the cost is nothing and the invariant is worth more:
  // there is exactly one order, readable directly, with no gaps to reason about
  // and no fractional ranks that drift toward collision.
  //
  // The CURSOR is server state, and it is intent rather than fact. It records
  // what the operator means to do next; what is actually on air is the node's
  // answer and lives nowhere in this schema. The two are allowed to disagree,
  // and THEORY-OF-OPERATIONS says the UI must show that divergence rather than
  // silently re-fire.
  (d) => {
    d.exec(`
      CREATE TABLE playlists (
        id         TEXT PRIMARY KEY,
        name       TEXT NOT NULL,
        created_at INTEGER NOT NULL,
        updated_at INTEGER NOT NULL
      );

      CREATE TABLE playlist_items (
        playlist_id TEXT NOT NULL REFERENCES playlists(id) ON DELETE CASCADE,
        item_id     TEXT NOT NULL REFERENCES items(id) ON DELETE CASCADE,
        position    INTEGER NOT NULL,
        PRIMARY KEY (playlist_id, item_id)
      );
      CREATE INDEX playlist_items_order ON playlist_items(playlist_id, position);

      CREATE TABLE rundowns (
        id         TEXT PRIMARY KEY,
        name       TEXT NOT NULL,
        created_at INTEGER NOT NULL,
        updated_at INTEGER NOT NULL
      );

      CREATE TABLE rundown_playlists (
        rundown_id  TEXT NOT NULL REFERENCES rundowns(id) ON DELETE CASCADE,
        playlist_id TEXT NOT NULL REFERENCES playlists(id) ON DELETE CASCADE,
        position    INTEGER NOT NULL,
        PRIMARY KEY (rundown_id, playlist_id)
      );
      CREATE INDEX rundown_playlists_order ON rundown_playlists(rundown_id, position);

      CREATE TABLE rundown_cursor (
        rundown_id TEXT PRIMARY KEY REFERENCES rundowns(id) ON DELETE CASCADE,
        item_id    TEXT,
        updated_at INTEGER NOT NULL
      );
    `)
  },

  // 5 — stations, users, groups, permissions, sessions, audit (ADR 0013).
  //
  // No PIN and no token is stored in a form that could be handed back: PINs are
  // scrypt-hashed with a per-user salt, and session and station tokens are
  // stored only as their SHA-256. A stolen copy of this file does not yield a
  // live session.
  (d) => {
    d.exec(`
      CREATE TABLE users (
        id           TEXT PRIMARY KEY,
        username     TEXT NOT NULL UNIQUE,
        display_name TEXT,
        pin_salt     TEXT NOT NULL,
        pin_hash     TEXT NOT NULL,
        created_at   INTEGER NOT NULL
      );

      CREATE TABLE groups (
        id         TEXT PRIMARY KEY,
        name       TEXT NOT NULL UNIQUE,
        created_at INTEGER NOT NULL
      );

      CREATE TABLE group_permissions (
        group_id   TEXT NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
        permission TEXT NOT NULL,
        PRIMARY KEY (group_id, permission)
      );

      CREATE TABLE user_groups (
        user_id  TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
        group_id TEXT NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
        PRIMARY KEY (user_id, group_id)
      );

      -- A station is an identity for the audit log, and a scope if someone
      -- later marks it trusted. Registration grants nothing.
      CREATE TABLE stations (
        id         TEXT PRIMARY KEY,
        name       TEXT NOT NULL,
        token_hash TEXT NOT NULL UNIQUE,
        trusted    INTEGER NOT NULL DEFAULT 0,
        scope      TEXT,
        created_at INTEGER NOT NULL
      );

      CREATE TABLE sessions (
        token_hash TEXT PRIMARY KEY,
        user_id    TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
        station_id TEXT,
        created_at INTEGER NOT NULL,
        expires_at INTEGER NOT NULL
      );

      CREATE TABLE audit_log (
        id         INTEGER PRIMARY KEY AUTOINCREMENT,
        at_utc     INTEGER NOT NULL,
        user_id    TEXT,
        station_id TEXT,
        action     TEXT NOT NULL,
        result     TEXT NOT NULL,
        detail     TEXT
      );
      CREATE INDEX audit_log_recent ON audit_log(at_utc DESC);
    `)
  },
]

/**
 * Open (and migrate) the database.
 *
 * `:memory:` is supported and is what the tests use — WAL is meaningless there
 * and better-sqlite3 ignores the pragma, which is fine.
 */
export function open(file = process.env.PMG_DB ?? 'var/pmg.db') {
  if (db) return db

  if (file !== ':memory:') fs.mkdirSync(path.dirname(path.resolve(file)), { recursive: true })
  db = new Database(file)
  db.pragma('journal_mode = WAL')
  // Content lives in the CAS, not here, so this database stays small enough
  // that NORMAL synchronous is the right trade: a crash can lose the last
  // transaction, and the last transaction is never a frame on air.
  db.pragma('synchronous = NORMAL')
  db.pragma('foreign_keys = ON')

  const at = db.pragma('user_version', { simple: true })
  for (let v = at; v < MIGRATIONS.length; v++) {
    db.transaction(() => {
      MIGRATIONS[v](db)
      db.pragma(`user_version = ${v + 1}`)
    })()
  }
  return db
}

export function handle() {
  return db ?? open()
}

/** Test seam. */
export function _reset() {
  if (db) db.close()
  db = null
}
