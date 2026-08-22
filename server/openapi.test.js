/**
 * The OpenAPI document, checked against the routes that actually exist.
 *
 * A hand-written API document drifts within a week and then actively misleads —
 * an integrator writing a Companion module against it has no way to tell a
 * documented path from a fictional one. So the document is checked BOTH ways:
 *
 *   1. Every documented path must exist in the app.
 *   2. Every route in the app must be documented, or listed below as
 *      deliberately outside the control API.
 *
 * The second half is the one that earns its keep. A new route lands, this
 * fails, and somebody decides whether integrators are meant to call it — which
 * is a decision, made once, rather than an omission nobody notices.
 *
 * The document is JSON rather than YAML so this test needs no parser and the
 * repo needs no new dependency. OpenAPI is defined for both.
 */

import assert from 'node:assert/strict'
import test, { before } from 'node:test'
import fs from 'node:fs'
import path from 'node:path'

let app, spec

/**
 * Routes that exist and are deliberately NOT part of the documented control
 * surface. Each needs a reason, because "not documented" and "not for you" are
 * different statements and only one of them is a decision.
 */
const NOT_PUBLIC = new Map([
  ['GET /api/health/public', 'Unauthenticated liveness for a load balancer. No content.'],
  ['GET /api/auth/me', 'Session plumbing for the browser (ADR 0013).'],
  ['POST /api/auth/login', 'Session plumbing for the browser.'],
  ['POST /api/auth/logout', 'Session plumbing for the browser.'],
  ['GET /api/users', 'Administration, not operation.'],
  ['POST /api/users', 'Administration, not operation.'],
  ['POST /api/groups', 'Administration, not operation.'],
  ['GET /api/stations', 'Administration, not operation.'],
  ['POST /api/stations/register', 'Administration, not operation.'],
  ['PUT /api/stations/:id', 'Administration, not operation.'],
  ['PUT /api/stations/:id/trust', 'Administration, not operation.'],
  ['GET /api/audit', 'Administration, not operation.'],
  ['GET /cas/:hash', 'Content-addressed asset transfer — plain HTTP with Range, deliberately NOT on the control socket (ADR 0007). Its own spec: asset-cas.md.'],
  ['POST /api/templates/:id/update-items', 'Design’s revision workflow — moving pinned Items forward is a browser action; integrators pin per Item via PATCH /api/items.'],
  ['GET /api/themes', 'Design’s theme workflow (ADR 0021) — themes resolve at compile in the browser; integrators see their effect as ordinary revisions.'],
  ['POST /api/themes', 'Design’s theme workflow (ADR 0021).'],
  ['GET /api/themes/:id/followers', 'Design’s theme workflow (ADR 0021) — what the apply sweep iterates.'],
  ['HEAD /cas/:hash', 'As above.'],
  ['GET /api/assets', 'Asset management, used by Design.'],
  ['GET /api/assets/sync', 'Per-node asset possession for the Media Browser. Derived view, not a contract.'],
  ['POST /api/assets', 'Asset management, used by Design.'],
  ['POST /api/assets/ingest', 'Server-side transcode on ingest. Design and Edit only.'],
  ['PATCH /api/assets/:hash', 'Asset management, used by Design — the label in a picker, never the identity.'],
  ['GET /api/ingest', 'Ingest job status. Design and Edit only.'],
  ['GET /api/ingest/:id', 'Ingest job status. Design and Edit only.'],
  ['POST /api/templates', 'Authoring. Design writes these; an integrator reads them.'],
  ['GET /api/templates/:id', 'Authoring detail.'],
  ['GET /api/templates/:id/revisions', 'Authoring detail.'],
  ['POST /api/templates/:id/publish', 'Authoring. Publishing is a Design act.'],
  ['POST /api/items', 'Editing. An integrator TAKES items; it does not build them.'],
  ['POST /api/items/:id/copy', 'Editing — a copy is a new Item (ADR 0026).'],
  ['POST /api/items/:id/move', 'Editing.'],
  ['GET /api/tags', 'Library organisation, used by Edit.'],
  ['PUT /api/playlists/:id/tags', 'Library organisation, used by Edit.'],
  ['GET /api/items/:id', 'Editing detail.'],
  ['PATCH /api/items/:id', 'Editing.'],
  ['DELETE /api/items/:id', 'Editing.'],
  ['POST /api/playlists', 'Editing.'],
  ['GET /api/playlists/:id', 'Editing detail.'],
  ['PUT /api/playlists/:id/items', 'Editing.'],
  ['PUT /api/playlists/:id/mode', 'Editing — changes what the arrow keys do (ADR 0017).'],
  ['DELETE /api/playlists/:id', 'Editing.'],
  ['POST /api/rundowns', 'Editing.'],
  ['PATCH /api/rundowns/:id', 'Editing — renaming a running order.'],
  ['DELETE /api/rundowns/:id', 'Editing.'],
  ['PATCH /api/playlists/:id', 'Editing — renaming a playlist.'],
  ['PUT /api/rundowns/:id/playlists', 'Editing.'],
  ['GET /api/nodes/:id/output-config', 'Configure — a node\u2019s hardware (ADR 0024). Integrators fire cues; what a node drives is plant configuration.'],
  ['PUT /api/nodes/:id/output-config', 'Configure. Reconfiguring an output closes a device and opens it again.'],
  ['GET /api/screens', 'Configure — a screen is plant configuration, like a node or an output (ADR 0023). Integrators fire cues at Items, and an Item already carries where it goes.'],
  ['POST /api/screens', 'Configure. Naming the places in a building is a human act.'],
  ['PATCH /api/screens/:id', 'Configure.'],
  ['DELETE /api/screens/:id', 'Configure.'],
  ['GET /api/nodes/:id', 'Configure detail.'],
  ['POST /api/nodes/:id/adopt', 'Configure. Adoption is a deliberate human act.'],
  ['GET /api/nodes/:id/outputs', 'Configure detail.'],
  ['PUT /api/nodes/:id/outputs/:out', 'Configure.'],
  ['POST /api/nodes/:id/outputs/:out/test-pattern', 'Configure — an instrument, not a cue.'],
])

/** Only the paths served by the CONTROL SERVER; the rest are the node's. */
const NODE_ONLY = new Set(['/cue', '/status'])

before(async () => {
  process.env.PMG_DB = ':memory:'
  process.env.PMG_CAS = fs.mkdtempSync('/tmp/pmg-openapi-')
  app = (await import('./index.js')).createApp()
  spec = JSON.parse(
    fs.readFileSync(path.join(import.meta.dirname, '..', 'docs', 'specs', 'openapi.json'), 'utf8'),
  )
})

/** Every `METHOD /path` the app actually serves, with Express's `:param` form. */
function routes() {
  const out = new Set()
  const walk = (stack) => {
    for (const l of stack) {
      if (l.route) {
        for (const m of Object.keys(l.route.methods)) out.add(`${m.toUpperCase()} ${l.route.path}`)
      } else if (l.name === 'router' && l.handle?.stack) walk(l.handle.stack)
    }
  }
  // Express 4 kept the stack on `_router`; 5 exposes `router`. Reading both is
  // what makes this test survive the upgrade rather than silently walking
  // nothing and passing.
  // oxlint-disable-next-line no-underscore-dangle
  walk(app.router?.stack ?? app._router.stack)
  return out
}

/** OpenAPI writes `{id}`; Express writes `:id`. */
const toExpress = (p) => p.replace(/\{(\w+)\}/g, ':$1')

test('every documented path exists', () => {
  const real = routes()
  const missing = []
  for (const [p, methods] of Object.entries(spec.paths)) {
    if (NODE_ONLY.has(p)) continue
    for (const m of Object.keys(methods)) {
      const key = `${m.toUpperCase()} ${toExpress(p)}`
      if (!real.has(key)) missing.push(key)
    }
  }
  // A documented route that does not exist is worse than an undocumented one:
  // an integrator writes against it and finds out on a Sunday.
  assert.deepEqual(missing, [], `documented but not served: ${missing.join(', ')}`)
})

test('every route is documented or deliberately excluded', () => {
  const documented = new Set()
  for (const [p, methods] of Object.entries(spec.paths)) {
    if (NODE_ONLY.has(p)) continue
    for (const m of Object.keys(methods)) documented.add(`${m.toUpperCase()} ${toExpress(p)}`)
  }
  const undecided = [...routes()].filter((r) => !documented.has(r) && !NOT_PUBLIC.has(r))
  // Not "document everything" — a decision about whether integrators are meant
  // to call it. Add it to the spec, or to NOT_PUBLIC with a reason.
  assert.deepEqual(
    undecided.toSorted(),
    [],
    `undocumented and unexcluded — add to openapi.json or to NOT_PUBLIC with a reason:\n  ${undecided.join('\n  ')}`,
  )
})

test('the exclusion list has no ghosts', () => {
  // A route that was deleted leaves its excuse behind, and the list slowly
  // becomes fiction. This is the other direction of the same drift.
  const real = routes()
  const gone = [...NOT_PUBLIC.keys()].filter((r) => !real.has(r))
  assert.deepEqual(gone, [], `excluded but no longer served: ${gone.join(', ')}`)
})

test('every documented op is one the server actually accepts', () => {
  // The op enum is the vocabulary every adapter is written against, so a name
  // in it that the server refuses is a Companion button that cannot work.
  const documented = spec.components.schemas.CueRequest.properties.op.enum
  const src = fs.readFileSync(path.join(import.meta.dirname, 'routes', 'cue.js'), 'utf8')
  const unhandled = documented.filter((op) => !src.includes(`case '${op}'`))
  assert.deepEqual(unhandled, [], `documented but not handled in buildOp: ${unhandled.join(', ')}`)
})
