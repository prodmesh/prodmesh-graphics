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
import { newId } from './ids.js'

/**
 * The schema version, which is the number of migrations. Derived rather than
 * declared: a hand-maintained constant beside a growing array is a number that
 * drifts, and this one had — it said 7 while fourteen migrations existed.
 */
export const schemaVersion = () => MIGRATIONS.length

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

  // 6 — per-output colour.
  //
  // `key_range` is its own column rather than folded into `range`: a plant with
  // a narrow-range fill and a full-range key is a real configuration, and one
  // column would make it unrepresentable (color-and-key.md §4).
  (d) => {
    d.exec(`
      ALTER TABLE outputs ADD COLUMN matrix    TEXT NOT NULL DEFAULT 'bt709';
      ALTER TABLE outputs ADD COLUMN range     TEXT NOT NULL DEFAULT 'narrow';
      ALTER TABLE outputs ADD COLUMN key_range TEXT NOT NULL DEFAULT 'narrow';
      ALTER TABLE outputs ADD COLUMN key_mode  TEXT NOT NULL DEFAULT 'fill';
    `)
  },

  // 7 — media metadata on an asset.
  //
  // Kept as JSON rather than columns: it is descriptive, it varies by media
  // type, and nothing queries across it. Columns would be a schema change every
  // time ffprobe learns a new field.
  (d) => {
    d.exec(`ALTER TABLE assets ADD COLUMN media TEXT`)
  },

  // 8 — an Item has several destinations (ADR 0018).
  //
  // The template moves out of the Item's identity and into its ROUTING. Same
  // data — the name, the title, the two lines of the verse — rendered by a
  // different template to each place it goes: a narrow strip over the key feed,
  // full-bleed type on the LED wall.
  //
  // `items.template_rev` and `items.destination` are left in place rather than
  // dropped. They are the one-destination answer, every existing row already
  // has them, and SQLite would rewrite the table to remove them. They stay as
  // the migration's source of truth and are no longer read once a row has been
  // expanded below.
  (d) => {
    d.exec(`
      CREATE TABLE item_destinations (
        item_id      TEXT NOT NULL REFERENCES items(id) ON DELETE CASCADE,
        -- Stable order, so "the first destination" means something to an
        -- operator reading a row and to an API returning one.
        ordinal      INTEGER NOT NULL,
        -- Which node, or NULL for "whichever node has this output". Null is the
        -- common case with one node and the honest answer before Configure can
        -- name them.
        node_id      TEXT,
        output       TEXT NOT NULL,
        layer        TEXT NOT NULL,
        -- PINNED per destination, for the same reason it was pinned per Item.
        template_rev TEXT NOT NULL REFERENCES template_revisions(template_rev),
        -- 'auto'       fires with the take
        -- 'on_request' available, but only when asked for — the full-screen
        --              version of a lower third that is taken "if we need it"
        mode         TEXT NOT NULL DEFAULT 'auto',
        -- This destination renders NOTHING for this Item, deliberately.
        -- An instruction, not an absence: full-screen graphics blank the LED
        -- wall and leave the background running, and clearing that feed is part
        -- of what taking this graphic means.
        blank        INTEGER NOT NULL DEFAULT 0,
        PRIMARY KEY (item_id, ordinal)
      );
      CREATE INDEX item_destinations_by_item ON item_destinations(item_id);

      -- Every existing Item becomes a list of one. Nothing changes for them.
      INSERT INTO item_destinations (item_id, ordinal, node_id, output, layer,
                                     template_rev, mode, blank)
        SELECT id, 0, NULL,
               COALESCE(json_extract(destination, '$.output'), 'out-0'),
               COALESCE(json_extract(destination, '$.layer'),  'Lower Third'),
               template_rev, 'auto', 0
          FROM items;
    `)
  },

  // 9 — a playlist decides whether navigating is taking (ADR 0017).
  //
  // On the PLAYLIST, not the node, the output, the rundown, or a preference on
  // someone's screen. A song is followed and a cuelist is cued — that is a fact
  // about the content, so it belongs to the content. As an operator preference
  // it would mean two stations disagreeing about whether clicking fires, with
  // the answer depending on who logged in last.
  //
  // `cue` is the default because it is the safe one: it has an arm step, and a
  // playlist that silently became `follow` would put a graphic on air the first
  // time someone clicked a row to look at it.
  (d) => {
    d.exec(`ALTER TABLE playlists ADD COLUMN mode TEXT NOT NULL DEFAULT 'cue';`)
  },

  // 10 — a playlist entry belongs to a named section.
  //
  // On the ENTRY rather than on the Item, because a section is a fact about
  // the arrangement and not about the content: the same chorus slide is sung
  // in Chorus 1 and again in Chorus 2. Reading a real ProPresenter document
  // makes that concrete — its arrangement held 31 slots over 19 slides.
  //
  // Nullable, because a sermon deck has no sections and inventing one for it
  // would be structure the document does not have.
  (d) => {
    d.exec(`ALTER TABLE playlist_items ADD COLUMN section TEXT;`)
  },

  // 11 — themes (ADR 0021).
  //
  // The canonical bytes live in the CAS under head_hash, hashed by the same
  // Rust encoder that makes template_rev mean something; the tokens JSON here
  // is a LISTING convenience the server never interprets. head_hash moves on
  // every save because a theme's identity is its content, like everything
  // else — templates record which hash their token values were resolved
  // under, so "compiled under which theme" always has one answer.
  (d) => {
    d.exec(`
      CREATE TABLE themes (
        id          TEXT PRIMARY KEY,
        name        TEXT NOT NULL,
        head_hash   TEXT NOT NULL,
        tokens      TEXT NOT NULL,
        created_at  INTEGER NOT NULL,
        updated_at  INTEGER NOT NULL
      );
    `)
  },

  // 12 — every hash a theme has ever had (ADR 0021, the apply sweep).
  //
  // A template's manifest records the theme HASH its values were resolved
  // under, and head_hash moves on every save — so "which templates follow
  // this theme" needs the trail, not just the head. Immutable rows: a hash's
  // meaning never changes, like every other content address here.
  (d) => {
    d.exec(`
      CREATE TABLE theme_hashes (
        hash      TEXT PRIMARY KEY,
        theme_id  TEXT NOT NULL
      );
      INSERT INTO theme_hashes (hash, theme_id) SELECT head_hash, id FROM themes;
    `)
  },

  // 13 — screens: a named place in the building (ADR 0023).
  //
  // The wiring moves out of every Item and into one row an operator can edit.
  // `template_id`, deliberately NOT `template_rev`: the screen supplies the
  // default treatment and the ITEM still pins the revision at save time, or a
  // publish would silently change every graphic that goes there — which is the
  // exact failure pinning exists to prevent.
  //
  // `item_destinations.screen_id` is added beside the existing triple rather
  // than replacing it. The triple stays as the resolution FALLBACK for a row
  // orphaned by database surgery, so a graphic goes where it used to go rather
  // than nowhere; deleting a screen Items point at is refused in the route.
  (d) => {
    d.exec(`
      CREATE TABLE screens (
        id          TEXT PRIMARY KEY,
        name        TEXT NOT NULL,
        -- NULL means "whichever node has this output", the same answer
        -- item_destinations already gives and the honest one with one node.
        node_id     TEXT,
        output      TEXT NOT NULL,
        layer       TEXT NOT NULL,
        -- The default treatment for graphics going here. NULL means the Item's
        -- own template, which is what every existing destination does today.
        template_id TEXT REFERENCES templates(id),
        created_at  INTEGER NOT NULL,
        updated_at  INTEGER NOT NULL
      );
      CREATE UNIQUE INDEX screens_by_name ON screens(name);

      ALTER TABLE item_destinations ADD COLUMN screen_id TEXT REFERENCES screens(id);
      CREATE INDEX item_destinations_by_screen ON item_destinations(screen_id);
    `)
  },

  // 14 — the hardware an operator asked each node for (ADR 0024).
  //
  // The NODE owns its outputs and persists them itself; this is the server's
  // record of what it last asked for, so Configure has something to show
  // before a node connects and something to re-send when one does. The node's
  // own file is the authority — if the two disagree, the node is right and its
  // StateReport says so.
  (d) => {
    d.exec(`
      CREATE TABLE node_outputs (
        node_id    TEXT NOT NULL,
        output_id  TEXT NOT NULL,
        config     TEXT NOT NULL,
        ordinal    INTEGER NOT NULL DEFAULT 0,
        updated_at INTEGER NOT NULL,
        PRIMARY KEY (node_id, output_id)
      );
    `)
  },

  // 15 — an asset's name.
  //
  // A hash is the right IDENTITY and a terrible name. `4a3b0d7b` in a picker
  // tells a designer nothing about which of four lower-third plates it is, and
  // the file arrived with a perfectly good name that the CAS threw away.
  //
  // A column, not the `media` JSON: this one IS queried and edited on its own,
  // and it applies to a font and an image as much as to a clip. It stays
  // nullable — the hash remains the fallback, and a name is a convenience that
  // must never be required for a render.
  (d) => {
    d.exec(`ALTER TABLE assets ADD COLUMN name TEXT`)
    // Clips ingested before this carried their filename inside the media blob.
    // Lifting it means an existing library reads correctly on the first boot
    // after the migration rather than starting over as hashes.
    for (const row of d.prepare(`SELECT hash, media FROM assets WHERE media IS NOT NULL`).all()) {
      try {
        const name = JSON.parse(row.media)?.filename
        if (name) d.prepare(`UPDATE assets SET name = ? WHERE hash = ?`).run(String(name), row.hash)
      } catch {
        // A media blob that will not parse is not a reason to fail a migration.
      }
    }
  },

  // 16 — an Item belongs to ONE playlist, and a playlist may repeat one.
  //
  // Two changes that are really one idea, which is why they land together
  // (ADR 0026).
  //
  // **Ownership.** An Item was free-floating and joined to playlists many to
  // many, so "All items" was the only complete view of the library — and at a
  // hundred songs of thirty slides each, that view is thousands of rows with
  // no structure. An Item now belongs to the playlist it was written for.
  // Copying one into another playlist makes a second Item, which is the honest
  // answer: two songs that both say "Verse 1" are two pieces of content, and
  // editing one must not silently change the other.
  //
  // **Repeats.** Inside ONE playlist the opposite is true. A chorus that comes
  // round three times is one piece of content in three places, and fixing a
  // typo in it must fix all three. The old primary key `(playlist_id,
  // item_id)` made that impossible to express. Entries get their own id so the
  // same Item can appear more than once — and the CURSOR moves to entries,
  // because "the cursor is on the chorus" stops being an answer the moment the
  // chorus is in the running order twice.
  (d) => {
    d.exec(`
      CREATE TABLE playlist_entries (
        entry_id    TEXT PRIMARY KEY,
        playlist_id TEXT NOT NULL REFERENCES playlists(id) ON DELETE CASCADE,
        item_id     TEXT NOT NULL REFERENCES items(id) ON DELETE CASCADE,
        position    INTEGER NOT NULL,
        section     TEXT
      );
      CREATE INDEX playlist_entries_order ON playlist_entries(playlist_id, position);
    `)
    const entry = d.prepare(
      `INSERT INTO playlist_entries (entry_id, playlist_id, item_id, position, section)
       VALUES (?, ?, ?, ?, ?)`,
    )
    for (const r of d.prepare('SELECT * FROM playlist_items ORDER BY playlist_id, position').all()) {
      entry.run(newId(), r.playlist_id, r.item_id, r.position, r.section ?? null)
    }

    d.exec(`ALTER TABLE items ADD COLUMN playlist_id TEXT REFERENCES playlists(id) ON DELETE CASCADE`)

    // An Item in several playlists becomes several Items — one per playlist,
    // which is what "belongs to one" MEANS. The first keeps the id, so every
    // rundown cursor, cue log entry and integrator `take_id` that already names
    // it still resolves.
    const copyItem = d.prepare(
      `INSERT INTO items (id, take_id, label, template_id, template_rev, data, destination,
                          created_at, updated_at, playlist_id)
       SELECT ?, NULL, label, template_id, template_rev, data, destination, created_at, ?, ?
         FROM items WHERE id = ?`,
    )
    const copyDests = d.prepare(
      `INSERT INTO item_destinations (item_id, ordinal, node_id, output, layer, template_rev,
                                      mode, blank, screen_id)
       SELECT ?, ordinal, node_id, output, layer, template_rev, mode, blank, screen_id
         FROM item_destinations WHERE item_id = ?`,
    )
    const now = Date.now()
    for (const item of d.prepare('SELECT id FROM items').all()) {
      const homes = d
        .prepare('SELECT DISTINCT playlist_id FROM playlist_entries WHERE item_id = ? ORDER BY playlist_id')
        .all(item.id)
        .map((r) => r.playlist_id)
      if (homes.length === 0) continue
      d.prepare('UPDATE items SET playlist_id = ? WHERE id = ?').run(homes[0], item.id)
      for (const other of homes.slice(1)) {
        const copy = newId()
        copyItem.run(copy, now, other, item.id)
        copyDests.run(copy, item.id)
        d.prepare('UPDATE playlist_entries SET item_id = ? WHERE playlist_id = ? AND item_id = ?')
          .run(copy, other, item.id)
      }
    }

    // An Item in NO playlist has nowhere to live under the new rule, and
    // deleting somebody's content to satisfy a schema is not a migration. They
    // go somewhere named, visible, and ordinary.
    const orphans = d.prepare('SELECT id FROM items WHERE playlist_id IS NULL').all()
    if (orphans.length) {
      const unfiled = newId()
      d.prepare('INSERT INTO playlists (id, name, mode, created_at, updated_at) VALUES (?, ?, ?, ?, ?)')
        .run(unfiled, 'Unfiled', 'cue', now, now)
      orphans.forEach((o, i) => {
        d.prepare('UPDATE items SET playlist_id = ? WHERE id = ?').run(unfiled, o.id)
        entry.run(newId(), unfiled, o.id, i, null)
      })
    }

    // The cursor addresses an ENTRY. With repeats, "the cursor is on item X"
    // finds the first X — so advancing past the second chorus would jump
    // backwards to the first.
    d.exec(`ALTER TABLE rundown_cursor ADD COLUMN entry_id TEXT`)
    for (const c of d.prepare('SELECT * FROM rundown_cursor').all()) {
      const found = d
        .prepare(
          `SELECT pe.entry_id FROM rundown_playlists rp
             JOIN playlist_entries pe ON pe.playlist_id = rp.playlist_id
            WHERE rp.rundown_id = ? AND pe.item_id = ?
            ORDER BY rp.position, pe.position LIMIT 1`,
        )
        .get(c.rundown_id, c.item_id)
      if (found) {
        d.prepare('UPDATE rundown_cursor SET entry_id = ? WHERE rundown_id = ?')
          .run(found.entry_id, c.rundown_id)
      }
    }

    d.exec(`DROP TABLE playlist_items`)
  },

  // 17 — a playlist's tags.
  //
  // A hundred songs is a hundred playlists, and a rail of a hundred names is a
  // scroll. Tags are on the PLAYLIST rather than the Item because that is what
  // the rail filters: tagging all thirty slides of a song "Christmas" would be
  // thirty edits saying one thing.
  //
  // Free strings, no tag table. A tag list nobody can add to without an admin
  // screen is a tag list nobody uses; the set is whatever has been typed, and
  // it is offered back as suggestions.
  (d) => {
    d.exec(`
      CREATE TABLE playlist_tags (
        playlist_id TEXT NOT NULL REFERENCES playlists(id) ON DELETE CASCADE,
        tag         TEXT NOT NULL,
        PRIMARY KEY (playlist_id, tag)
      );
      CREATE INDEX playlist_tags_tag ON playlist_tags(tag);
    `)
  },

  // 18 — named arrangements (ADR 0027).
  //
  // The playlist's own entry order is **Main**: every slide, in the song's
  // order, with its groups named by `section`. An arrangement is an ordered
  // list of GROUP NAMES over that — "Chorus, Verse 1, Chorus" — and it holds
  // no slides of its own.
  //
  // That is the whole point. An arrangement that stored slides would be a
  // second copy of the song, and the first typo would prove it: fixing the
  // chorus in Main would leave every arrangement still singing the old words.
  // Naming groups instead means an arrangement follows Main by construction —
  // add a line to the chorus and every arrangement using it grows one.
  (d) => {
    d.exec(`
      CREATE TABLE arrangements (
        id          TEXT PRIMARY KEY,
        playlist_id TEXT NOT NULL REFERENCES playlists(id) ON DELETE CASCADE,
        name        TEXT NOT NULL,
        created_at  INTEGER NOT NULL
      );
      CREATE INDEX arrangements_playlist ON arrangements(playlist_id, name);

      CREATE TABLE arrangement_groups (
        arrangement_id TEXT NOT NULL REFERENCES arrangements(id) ON DELETE CASCADE,
        position       INTEGER NOT NULL,
        section        TEXT NOT NULL,
        PRIMARY KEY (arrangement_id, position)
      );

      -- Which arrangement THIS service wants of THIS song. NULL is Main, which
      -- is what every existing row means and what most rows will keep meaning.
      ALTER TABLE rundown_playlists
        ADD COLUMN arrangement_id TEXT REFERENCES arrangements(id) ON DELETE SET NULL;
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
