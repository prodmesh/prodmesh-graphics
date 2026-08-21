/**
 * Screens — a named place in the building (ADR 0023).
 *
 * "LED Wall", not `booth-1 / wall / Graphics`. Configured once, so moving a
 * wall to another node is one edit instead of one per Item, and an operator
 * building an Item is asked about the room rather than about the wiring.
 *
 * **Resolution lives here and nowhere else.** The assignment build and the cue
 * path both have to turn a destination into a node, an output, and a layer, and
 * the two disagreeing would mean an Item that is staged for one place and fired
 * at another. One function, called from both.
 *
 * The NODE never hears about any of this. Its contract is outputs and layers,
 * exactly as before, which is why screens needed no protocol change and work
 * against a node running yesterday's build.
 */

import { handle } from './db.js'

/** Every screen, in name order — how a person reads a list of places. */
export function all() {
  return handle()
    .prepare(
      `SELECT id, name, node_id, output, layer, template_id, created_at, updated_at
         FROM screens ORDER BY name`,
    )
    .all()
}

export function get(id) {
  return handle()
    .prepare(
      `SELECT id, name, node_id, output, layer, template_id, created_at, updated_at
         FROM screens WHERE id = ?`,
    )
    .get(id)
}

/**
 * Turn one stored destination row into where it actually goes.
 *
 * A row naming a screen resolves through it. A row that does not is already an
 * answer, and stays legal: a fresh install has to be able to address its own
 * hardware before anybody has invented a name for it.
 *
 * A screen that has gone missing falls back to the triple stored beside it, so
 * a graphic goes where it used to go rather than nowhere. That is a backstop
 * for a row orphaned by database surgery — the route refuses to delete a screen
 * Items point at — and `resolved` says which happened, so a caller can tell an
 * operator rather than quietly doing the wrong thing.
 */
export function resolve(row, screensById = null) {
  if (!row.screen_id) {
    return { ...row, screen_id: null, screen_name: null, resolved: 'direct' }
  }
  const s = screensById ? screensById.get(row.screen_id) : get(row.screen_id)
  if (!s) {
    return { ...row, screen_name: null, resolved: 'orphaned' }
  }
  return {
    ...row,
    node_id: s.node_id ?? null,
    output: s.output,
    layer: s.layer,
    screen_name: s.name,
    resolved: 'screen',
  }
}

/** Every screen by id, for resolving a list without a query per row. */
export function byId() {
  return new Map(all().map((s) => [s.id, s]))
}

/**
 * Resolve a list of destination rows in one pass.
 */
export function resolveAll(rows) {
  if (!rows.some((r) => r.screen_id)) return rows.map((r) => resolve(r))
  const map = byId()
  return rows.map((r) => resolve(r, map))
}

/**
 * The template revision a destination should pin when it is saved.
 *
 * The screen supplies the DEFAULT treatment and the Item still pins the
 * revision — a screen that supplied the revision at take time would make every
 * publish a silent change to every graphic that goes there, which is the exact
 * failure pinning exists to prevent (ADR 0023).
 *
 * Returns `null` when the screen names no template, or names one that has never
 * been published; the caller falls back to the Item's own, which is what every
 * destination did before screens existed.
 */
export function defaultRevFor(screenId) {
  if (!screenId) return null
  const row = handle()
    .prepare(
      `SELECT t.head_rev AS rev
         FROM screens s JOIN templates t ON t.id = s.template_id
        WHERE s.id = ?`,
    )
    .get(screenId)
  return row?.rev ?? null
}

/** Items whose destinations point at this screen, by label — for a refusal. */
export function usedBy(screenId) {
  return handle()
    .prepare(
      `SELECT DISTINCT i.label
         FROM item_destinations d JOIN items i ON i.id = d.item_id
        WHERE d.screen_id = ? ORDER BY i.label`,
    )
    .all(screenId)
    .map((r) => r.label)
}
