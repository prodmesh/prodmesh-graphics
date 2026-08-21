# ProdMesh Graphics — contributor / agent guide

A distributed broadcast graphics design and playout system for templated keyed
content. Designers build **Templates** in a browser; operators bind data to them
as **Items**, arrange them into **Playlists** and a **Rundown**, and take them on
air. **Playout Nodes** — native Rust processes on Mac, Linux, or Windows — render
those graphics to SDI, NDI, or a GPU display output, with key and fill.

The North Star: **the operator simplicity of ProPresenter, with the authoring
stability, performance, and maturity of Ross Xpression or Vizrt.**

Sibling projects whose conventions this repo follows deliberately:
`~/Desktop/church-production-dashboard` (ProdMesh-Control — the flagship; server
patterns, SSE topic hub, auth model, design tokens, ADR format) and
`~/Code/prodmesh-caption` / `~/Code/prodmesh-rta` (the C++ apps; `--selftest`
gating, the hand-written-header shim pattern, release ritual). Deviations from
both are listed under **Conventions** and each has a reason.

> **Status: M0, and its three gating spikes have all passed.** S1 (render parity)
> is bit-identical on Metal, S3 (DeckLink output) held 1080p59.94 for ten minutes
> with zero late frames, and S6 (WebGPU in the browser) works. No scene format
> and no server beyond health yet — but the renderer runs on both targets and
> **both vendor boundaries are real**: `sys/ndi-sys` sends and receives frames,
> `sys/decklink-sys` drives scheduled SDI output. Drive them with `pmgctl ndi`,
> `pmgctl decklink`, and `pmgctl render-golden`.
>
> Read `docs/ROADMAP.md` for what is being built and in what order,
> `docs/STATE.md` for what is actually real today, and
> `docs/INTEGRATION-NOTES.md` before re-deriving anything about a vendor SDK.

## Architecture

Start with `docs/ARCHITECTURE.md`, then `docs/THEORY-OF-OPERATIONS.md`. The
one-paragraph version: the **control server** owns content and assignment; a
**playout node** owns what is on air. Templates compile to a **SceneDoc**, get
content-addressed, and are pulled by the nodes that need them. Snapshots stream
over SSE; cues are acked over a dedicated socket. A node that loses the server
keeps playing.

### Rust workspace

| Crate | Contents |
|---|---|
| `crates/pmg-scene` | `SceneDoc` types, canonical CBOR + JSON mirror, validation, migration, `template_rev` hashing |
| `crates/pmg-layout` | Flexbox (Yoga model) solver over a `TextMeasure` trait |
| `crates/pmg-text` | Font DB, shaping, line breaking, glyph rasterization, atlas |
| `crates/pmg-anim` | Timelines, keyframe tracks, easing, evaluate-at-t |
| `crates/pmg-render` | wgpu render graph: `SceneInstance` → texture. **No I/O, no platform.** |
| `crates/pmg-color` | Primaries / transfer / matrix / range conversion, key-channel generation |
| `crates/pmg-media` | Image + video demux and decode, frame cache, A/V clock |
| `crates/pmg-audio` | Mix, resample, layer → output audio pair mapping |
| `crates/pmg-cas` | Content-addressed store: hash, put, get, verify, disk cache, GC |
| `crates/pmg-proto` | PMG Link message types — the wire contract of record |
| `crates/pmg-node` | The playout binary: link client, assignment reconciler, layer engine, scheduler |
| `crates/pmg-out-ndi` | NDI output backend (M1) |
| `crates/pmg-out-decklink` | DeckLink output backend (M2) |
| `crates/pmg-out-window` | GPU display output via winit (M2) |
| `crates/pmg-preview` | Downscale, encode, and ship the preview channel |
| `crates/pmg-wasm` | `wasm-bindgen` surface for Design mode in the browser |
| `crates/pmg-cli` | `pmgctl` — compile, inspect, render-golden, cas verify, chaos, `--selftest` |
| `sys/decklink-sys` | Hand-written C shim over the DeckLink C++ SDK, plus `build.rs` |
| `sys/ndi-sys` | NDI C SDK bindings, loaded at runtime (redistribution seam) |

### Server and web app

| Path | Contents |
|---|---|
| `server/index.js` | App assembly, static `dist/` in production |
| `server/db.js` | SQLite (WAL) schema and migrations |
| `server/streamHub.js` | SSE topic hub — snapshots only, conflating (see ADR 0008) |
| `server/nodeLink.js` | PMG Link server: node sessions, assignment, cue dispatch |
| `server/cueLog.js` | The ordered cue log and its `?since=<seq>` gap endpoint |
| `server/cas.js` | Content-addressed asset store and `GET /cas/:hash` with Range |
| `server/health.js` | `declare` / `report` / `snapshot` registry, privacy-filtered |
| `server/authStore.js` | Stations, users, groups, permissions (ADR 0013) |
| `src/modes/{configure,design,edit,show}/` | The four operator modes |
| `src/lib/{stream,useQuery,keys}.ts` | Push and pull data layers, shared cache keys |

### Two rules enforced by CI, not by discipline

1. **`pmg-render` has no I/O and no platform.** It takes a `SceneInstance`, a
   `wgpu::Device`, and a target texture. That is what lets it compile to WASM
   unchanged, and the shared renderer is the whole premise of ADR 0001.
2. **The pure set may not depend on tokio, the filesystem, or any vendor SDK.**
   The pure set is `pmg-scene`, `pmg-layout`, `pmg-text`, `pmg-anim`,
   `pmg-render`, `pmg-color`. Enforcement is a required
   `wasm32-unknown-unknown` build job — a lint would drift, a failing build
   cannot.

**The server never parses SceneDocs.** It stores them as opaque CAS blobs plus a
small JSON manifest sidecar that the Rust compiler emits, and validates only the
sidecar. A JavaScript reimplementation of the scene format would be two
implementations of the thing that must never disagree — the same discipline that
keeps `arrangeSlides()` a single expansion in ProdMesh-Control.

## Build & test

### Vendor SDKs

DeckLink and NDI are **licensed separately and never committed**
(`sys/*/vendor/` is gitignored — ADR 0003). Keep them in `~/Code/3P_SDKs/` and
symlink, so a cleared Downloads folder cannot turn into a confusing
"driver not loaded" morning:

```bash
ln -sfn ~/Code/3P_SDKs/"Blackmagic DeckLink SDK 16.0" sys/decklink-sys/vendor
```

`build.rs` also honours `DECKLINK_SDK_DIR`, but a symlink survives a new shell
and the env var does not. Without either, the shim builds a **stub** — a
supported configuration that CI relies on, so check both: a change to
`sys/decklink-sys` must compile with the vendor link moved aside.

```bash
cargo build --workspace                                  # all crates
cargo build -p pmg-render --target wasm32-unknown-unknown # proves the pure-set rule
cargo run -p pmg-cli -- --selftest                       # must print PASS
npm ci && npm run build && npm test
npm run dev                                              # server :8519 + Vite
```

- **Always run `pmgctl --selftest` after touching `pmg-scene`, `pmg-layout`,
  `pmg-anim`, or `pmg-color`.** It must print `PASS`; CI runs it and it gates
  releases, exactly as in prodmesh-caption and prodmesh-rta.
- **Golden images** live in `testdata/golden/`. `pmgctl render-golden` writes
  them; `.github/workflows/parity.yml` diffs native against WASM. A wgpu upgrade
  is a release-gated change requiring a full golden re-run.
- Ports: control server **8519**, node **8520**, preview **8521**. (Family 85xx
  band: RTA 8600, Caption 8518.)

## Gotchas

Each of these has burned real time somewhere, or is a trap the design is
specifically shaped to avoid. Do not re-derive them.

- **Shared code gives identical pixels — measured on Metal, and only there so
  far.** Spike S1 (2026-08-14) found native wgpu and browser WebGPU
  **bit-identical** across the parity corpus: 0/255 max delta, zero geometric
  displacement. But Chrome reported `vendor: apple, architecture: metal-3`, so
  that was two WGSL frontends over *one* driver. **D3D12 and Vulkan are
  untested** — a Windows designer against a Mac node is the open case. The rules
  that made it come out clean are not optional: **never MSAA** (sample patterns
  are unspecified), **never `fwidth`/`dpdx`/`dpdy`** for antialiasing (derivative
  granularity is implementation-defined — pass pixel scale as a uniform),
  **`Rgba8Unorm` not `Rgba8UnormSrgb`**, every sampler field stated, and one
  pinned wgpu version across both targets.
- **Never read pixels back through a canvas.** `toDataURL` may apply a colour
  profile, which turns a renderer comparison into a measurement of the browser's
  colour pipeline. The parity harness posts raw RGBA.
- **Genlock gives phase, not frame numbering.** Two nodes on the same reference
  output in lockstep but do not agree on which frame is number 1841944. v1
  promises phase-locked outputs *within* a node and does **not** promise
  frame-accurate simultaneous takes *across* nodes. See ADR 0011.
- **The key is not a colour.** Fill goes RGB→YCbCr through a 601/709/2020
  matrix. The key is a monotone alpha signal: write `Y = alpha` mapped to the
  configured range, `Cb = Cr =` neutral, and never put it through a colour
  matrix. Getting this wrong makes soft key edges subtly wrong downstream.
- **Composite in linear, premultiplied.** Compositing gamma-encoded values gives
  dark halos on white text over a dark key. Encode once, at output.
- **Never publish a bare event frame on an SSE topic.** `streamHub` conflates by
  design and a superseded event is a *lost* event. Events reach the browser as
  the snapshot of a log tail (`show:<id>:cuelog` carrying `{head_seq, entries[]}`),
  with `?since=<seq>` over plain HTTP for the gap. ADR 0008.
- **Assets never travel over the control socket.** A 4 GB video must not
  head-of-line-block a take. CAS transfers are plain HTTP with Range and resume;
  preview is a third, separate connection.
- **A node acks an `Assignment` only after persisting it to disk.** That
  ordering is the entire autonomy invariant in one line. ADR 0009.
- **Reject a late take; never fire it.** A graphic that appears four seconds
  after the button because the link hiccuped is worse than one that never
  appears. Cues carry a deadline.
- **Text shudders if you snap glyphs to integers.** Broadcast CG slides text
  horizontally. Cache glyphs at fractional offsets (⅛ px phases) or use SDFs.
  Never use subpixel-RGB antialiasing — it is meaningless over an alpha key, and
  the AA edge must be premultiplied correctly or you get fringes over live video.
- **`layout.*` forces a relayout; `transform.*` does not.** Animate transforms.
  A responsive lower third sizes itself *once*, when data binds, and reveals
  with a transform and a clip.
- **A node refuses a template it is too old to read at assignment time**, loudly,
  not at take time. You find out Saturday, not at 10:15 Sunday.
- **A vendor SDK newer than the installed driver reports capabilities as
  *defaults*, not as errors.** Desktop Video 14.5 does not implement SDK-16.0
  IIDs, so `QueryInterface` returns `E_NOINTERFACE` and a DeckLink Quad 2 reads
  as having no keying, no audio, and persistent id 0. `sys/decklink-sys` queries
  IIDs newest-first and falls back — which is safe **only** for methods at
  identical vtable slots. `IDeckLinkOutput` diverges at slot 6. Verify a
  method's slot in every version of the chain before calling it, or narrow the
  chain. Measured, with the vtable table, in `docs/INTEGRATION-NOTES.md`.
- **A CPU-generated 1080p frame misses 59.94.** Measured 17.55 ms against a
  16.68 ms budget for generate + unpremultiply + encode. Rasterize once at arm;
  animate transforms over the cached result; do the per-pixel encode on the GPU.
- **`cmd 2>&1 | grep ...; echo OK` reports OK when `cmd` failed.** The pipeline's
  status is grep's, and a trailing `echo` runs regardless — so a clippy or test
  failure scrolls past above a cheerful "CLEAN". Two commits landed with clippy
  errors in them this way. Check the exit status (`cmd >/dev/null 2>&1 && echo
  ok || echo FAILED`) rather than printing a constant.
- **A time that is not a time reaches the render thread.** `pmg_anim::eval`
  held before the first key and after the last with `<=` / `>=`, and NaN
  compares false against both — so it fell into the interval search and
  underflowed `partition_point() - 1` to `usize::MAX`, panicking the wasm
  module mid-frame. It arrived from a UI divide-by-zero (a collapsed panel with
  zero width), which is fixed in its own place, but the lesson is the general
  one: anything on the path to a frame must be TOTAL over its inputs, not
  merely correct over the sensible ones.
- **`npx tsc --noEmit` checks NOTHING in this repo.** The root `tsconfig.json`
  is a solution file — `"files": []` plus two project references — so that
  command type-checks an empty program and exits 0 looking like a pass. The
  real check is `npm run build`, which runs `tsc -b` across the references and
  includes the test files. Two type errors reached a rehearsal build this way,
  after `tsc --noEmit` had reported clean several times.
- **A permission boundary with only a server half is invisible until it
  isn't.** `requirePermission` was complete and tested from M2 — and the browser
  sent no station header, no bearer token, and had no sign-in anywhere. Nothing
  failed, because `unconfigured()` lets everything through until the first user
  exists. Adding that user turned enforcement on for good and left a console
  that could not be administered from itself. When a boundary is gated on
  "nobody has configured this yet", the client half is not optional work for
  later: it is the half that gets exercised the moment somebody uses the
  feature. Same shape as the footer that said "add a user in Configure" for
  four milestones while no such page existed.
- **A safety guard in `createUser` does not protect rows that predate it.** The
  first user gets the wildcard so that securing a server does not lock it — and
  the rehearsal account, created forty minutes after that landed in the tree by
  a server process started before it, held nothing at all. Sign-in worked and
  every action was refused. Any guard of the form "the first X is special" only
  covers Xs made by code that has it; the recovery path (`npm run grant-admin`)
  is the part that covers the rest, and the server now REPORTS
  `no_administrator` rather than letting it be discovered by refusal.
- **A browser that posts raw bytes bypasses every convention in `api.ts`.**
  Two uploads in Design built their own `fetch`, so they sent no credentials
  and rendered `the server answered 401` instead of the server's sentence.
  Anything that talks to the server goes through `src/lib/api.ts` — the headers
  and the refusal handling live there, once.
- **Long-GOP H.264 is where frame-accurate seek goes to die.** Require an
  all-intra intermediate (ProRes 4444, DNxHR, HAP) for anything that must hit a
  mark; the server transcodes on ingest.

## Conventions

- **Vocabulary is Template / Item / Playlist / Rundown**, in the UI, the docs,
  and the API. See `docs/UI_TEXT.md`; it is enforceable copy, not a preference.
- **Comments explain *why*, not what.** Measured facts are dated and
  hardware-attributed, and live in `docs/INTEGRATION-NOTES.md`.
- **Structural choices land as an ADR** in `docs/decisions/`, in the format the
  existing ones use, *plus* a `## Seam` section naming what the decision must not
  foreclose. That section is how "in-house now, product later" stays real.
- **No CDNs, ever.** Fonts and assets are bundled or served from the CAS. Booth
  machines have no internet. Inherited from the whole family.
- **Rust:** stable toolchain pinned in `rust-toolchain.toml`; `thiserror` for
  library errors, `anyhow` at binary edges; `tracing` for logging. No `unsafe`
  outside `sys/`.
- **Server:** plain ESM JavaScript, no build step, `better-sqlite3` with WAL,
  `node --test`. **Web:** React 19 + TypeScript + Vite, no component library,
  `lucide-react` icons, tokens in `src/styles/tokens.css` and never hard-coded
  hex in feature CSS. All inherited verbatim from ProdMesh-Control.
- **`_reset()` is the test-seam convention** for dropping a module's state
  between tests, inherited from ProdMesh-Control. The underscore is the signal
  that nothing in production may call it; it is allowlisted in `.oxlintrc.json`
  for exactly that name and no other.
- **Vendor SDKs get a hand-written C header that is the contract of record**, and
  a shim `.cpp` — never bindgen pointed at C++ virtual interfaces. This is the
  `prodmesh-caption` Swift-shim pattern (`swift/include/pmcaption_speech.h`),
  reused deliberately.
- **Git:** sentence-case imperative subjects describing the *effect*, prefixed
  `M0:`/`M1:` during build-out.

## Releases

Bump the workspace version in `Cargo.toml` and `package.json` together (server
and node ship as a matched pair), tag `vX.Y.Z`, and let CI verify the tag matches
the version before it builds. Plain pushes never release. macOS artifacts are
Developer ID signed and notarized from GitHub Actions secrets; certificates live
in `~/Code/SIGNING/`.

**This repo is private and all rights reserved.** See `LICENSE` and the posture
section of `docs/VISION.md` — relicensing is a deliberate later decision, and
defaulting to the family's MIT habit would foreclose it permanently.
