# Architecture

The mental model. Read `docs/THEORY-OF-OPERATIONS.md` first if you want the
narrative; this is the structural reference.

## The pieces

```
┌─ Browser ──────────────────────────────────────────────────────────┐
│  Configure · Design · Edit · Show    React 19 + TS + Vite          │
│  Design canvas runs pmg-render compiled to WASM/WebGPU             │
└────────┬──────────────────────────┬────────────────────────────────┘
         │ REST + SSE topics        │ preview stream (separate socket)
         ▼                          │
┌─ Control server :8519 ────────────┼────────────────────────────────┐
│  Express 5 (plain ESM) · SQLite WAL · CAS on disk                  │
│  streamHub (snapshots) · cueLog (events) · nodeLink · authStore    │
│  NEVER parses a SceneDoc                                           │
└────────┬────────────────────┬─────┼────────────────────────────────┘
         │ PMG Link           │ CAS │
         │ CBOR / WebSocket   │ HTTP│
         │ node dials in      │Range│
         ▼                    ▼     ▼
┌─ Playout node :8520 ───────────────────────────────────────────────┐
│  pmg-node: link client · assignment reconciler · layer engine      │
│  ┌── output group A (its own render thread, its own frame clock) ──┐│
│  │  layers: Fullscreen / Ticker / Lower Third / Bug                ││
│  │  pmg-render → composite → pmg-color → pmg-out-{ndi,decklink,…}  ││
│  └─────────────────────────────────────────────────────────────────┘│
│  local CAS · assignment.cbor · GPI/OSC/MIDI terminate HERE          │
└─────────────────────────────────────────────────────────────────────┘
```

Ports: control server **8519**, node **8520**, preview **8521**. Family 85xx
band — ProdMesh RTA is 8600, ProdMesh Caption 8518.

## Invariants — do not break these

1. **The node is the authority on what is on air; the server is the authority on
   content and assignment.** The server adopts `StateReport`; it never overwrites
   it. (ADR 0009)
2. **`pmg-render` has no I/O and no platform.** `SceneInstance` + `wgpu::Device`
   + target texture in, rendered texture out. This is what lets it compile to
   WASM unchanged. (ADR 0001, 0002)
3. **The pure set may not depend on tokio, the filesystem, or any vendor SDK.**
   Pure set: `pmg-scene`, `pmg-layout`, `pmg-text`, `pmg-anim`, `pmg-render`,
   `pmg-color`. Enforced by a required `wasm32-unknown-unknown` CI job, because a
   lint would drift and a failing build cannot. (ADR 0004)
4. **The server never parses a SceneDoc.** It stores opaque CAS blobs plus a
   compiler-emitted JSON manifest sidecar, and validates only the sidecar. A
   JavaScript reimplementation of the scene format would be two implementations
   of the thing that must never disagree. (ADR 0004)
5. **Snapshots stream, cues are acked.** Nothing event-shaped rides a conflating
   transport. Event feeds reach browsers as the snapshot of a log tail. (ADR 0008)
6. **Assets never travel over the control socket.** CAS is plain HTTP with Range
   and resume; preview is a third socket. (ADR 0006, 0007)
7. **A node acks an `Assignment` only after persisting it to disk.** (ADR 0009)
8. **A node refuses a template whose `min_reader` exceeds its own, at assignment
   time, loudly.** (ADR 0005)
9. **Composite in linear premultiplied float; encode once at output; the key
   never goes through a colour matrix.** (ADR 0012)
10. **Node ids in a SceneDoc are stable forever.** Timeline tracks address
    `node_id/property`, so reordering or renaming a layer can never break an
    animation. (ADR 0005)

## Repository layout

```
prodmesh-graphics/
  Cargo.toml  rust-toolchain.toml           workspace root, pinned toolchain
  package.json  vite.config.ts              one npm project: server + web UI
  crates/                                   17 crates, see table below
  sys/decklink-sys/  sys/ndi-sys/           vendor SDK boundaries, all unsafe lives here
  server/                                   Express 5, plain ESM, no build step
  src/                                      React 19 + TypeScript
  testdata/                                 golden templates, golden PNGs, migration fixtures
  docs/                                     you are here
  .github/workflows/                        build · parity · release
```

One repo, not several. The premise of ADR 0001 is that the designer and the node
run *the same renderer code*; a polyrepo turns "these must never drift" into a
version-matrix problem at exactly the point where drift is the failure mode. The
scene format has four consumers (designer WASM, node, server validation, CLI) and
they need one schema and one migration chain. Server and node ship as a matched
pair. The cost — heavier CI across Rust ×3 targets plus Node plus WASM — is paid
with path-filtered jobs.

### Crates

| Crate | Responsibility | Pure? |
|---|---|---|
| `pmg-scene` | SceneDoc types, canonical CBOR + JSON mirror, validation, migration, hashing | ✅ |
| `pmg-layout` | Flexbox (Yoga model) solver over a `TextMeasure` trait | ✅ |
| `pmg-text` | Font DB, shaping, line breaking, glyph rasterization, atlas | ✅ |
| `pmg-anim` | Timelines, keyframe tracks, easing, evaluate-at-t | ✅ |
| `pmg-render` | wgpu render graph: `SceneInstance` → texture | ✅ |
| `pmg-color` | Primaries / transfer / matrix / range, key-channel generation | ✅ |
| `pmg-media` | Image + video demux and decode, frame cache, A/V clock | |
| `pmg-audio` | Mix, resample, layer → output audio pair mapping | |
| `pmg-cas` | Content-addressed store: hash, put, get, verify, disk cache, GC | |
| `pmg-proto` | PMG Link message types — the wire contract of record | |
| `pmg-node` | Playout binary: link client, assignment reconciler, layer engine, scheduler | |
| `pmg-out-ndi` | NDI output backend (M1) | |
| `pmg-out-decklink` | DeckLink output backend (M2) | |
| `pmg-out-window` | GPU display output via winit (M2) | |
| `pmg-preview` | Downscale, encode, ship the preview channel | |
| `pmg-wasm` | `wasm-bindgen` surface for Design mode | |
| `pmg-cli` | `pmgctl`: compile, inspect, render-golden, cas verify, chaos, `--selftest` | |

"Pure" means invariant 3 applies and the crate is in the WASM build job.

### Server modules

| Module | Responsibility |
|---|---|
| `server/index.js` | App assembly; serves `dist/` in production |
| `server/db.js` | SQLite schema and migrations, WAL |
| `server/streamHub.js` | SSE topic hub — **snapshots only**, conflating, refcounted producers |
| `server/cueLog.js` | Ordered cue log, the `show:<id>:cuelog` tail topic, `?since=<seq>` gap endpoint |
| `server/nodeLink.js` | PMG Link server: sessions, assignment offers, cue dispatch, replay ring |
| `server/cas.js` | Content-addressed store, `GET /cas/:hash` with Range |
| `server/assignments.js` | Computes what each node needs from Items and Templates |
| `server/health.js` | `declare` / `report` / `snapshot`, privacy-filtered `publicHealth()` |
| `server/authStore.js` | Stations, users, groups, permissions (ADR 0013) |
| `server/httpAuth.js` | `requirePermission()` middleware, 401/403 split, audit |
| `server/routes/*.js` | Routers declaring full `/api/...` paths |

### Web app

`src/modes/{configure,design,edit,show}/` — the four modes from ADR 0015.
`src/lib/stream.ts` (`useTopic`, one `EventSource` per tab, refcounted,
reconnect debounced 30 ms), `src/lib/useQuery.ts` (request/response with a shared
cache), `src/lib/keys.ts` (cache keys shared between them). `src/styles/tokens.css`
holds every colour, radius, and font; feature CSS references tokens and never
hard-codes hex. No component library. `lucide-react` for icons. Fonts bundled via
`@fontsource` — **no CDNs, ever**, because booth machines have no internet.

All of this is ProdMesh-Control's shape, deliberately. An operator moving between
the two apps should not have to learn a second set of conventions, and we should
not have to invent a second set of answers to problems the flagship already
solved.

## Data model

```
Template ──1:n── TemplateRevision (immutable, id = blake3 of its SceneDoc)
                        │
                        └── referenced by ──┐
                                            │
Item { template_id, template_rev, data{}, destination{node, output, layer} }
  │
  └── ordered in ── Playlist ── ordered in ── Rundown

Node ──1:n── Output ──1:n── Layer (named, ordered, ≤1 live graphic each)

Asset (blake3) ── referenced by TemplateRevision and by Item data
```

**Items pin a revision.** Republishing a template never changes anything on air;
moving an Item to a newer revision is an explicit action with a diff.

**A layer holds at most one live graphic.** Taking a new lower third replaces
what is on the Lower Third layer and does not touch the Bug. `clear-layer` and
`clear-all` are first-class operations, not macros.

## Deployment

Three shapes, matching ProdMesh-Control so the ops knowledge transfers: a **git
checkout on a LAN box** under launchd/systemd, a **container** from GHCR with all
mutable state in one volume, or a **desktop launcher** that runs the server
in-process. The node is a separate binary installed per playout machine, with its
own service unit and its own local state directory.

State on the server: SQLite at `data/prodmesh-graphics.db`, CAS blobs under
`data/cas/`. State on the node: `assignment.cbor` and a local CAS. Nothing else
is needed to come up and play.

## Gotchas learned

Kept in `AGENTS.md` so they are in front of anyone doing the work, and expanded
in `docs/RISKS.md`. The short list, because they shaped this architecture:
shared code does not mean identical pixels; genlock gives phase, not frame
numbering; the key is not a colour; never snap animated glyphs to integer
positions; `layout.*` relayouts and `transform.*` does not; long-GOP H.264 cannot
be seeked frame-accurately.
