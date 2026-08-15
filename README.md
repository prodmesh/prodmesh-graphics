# ProdMesh Graphics

A distributed graphics design, programming, and playout system for templated
keyed broadcast content.

Design a **Template** in the browser. Bind data to it as an **Item**. Arrange
Items into **Playlists**, and Playlists into a **Rundown**. Take them on air.
**Playout Nodes** render them to SDI, NDI, or a GPU display output, with key and
fill, phase-locked to an external reference.

The North Star: **the operator simplicity of ProPresenter, with the authoring
stability, performance, and maturity of Ross Xpression or Vizrt.**

---

> ### Status: M0 — scaffold and documents
>
> There is no engine yet. The Cargo workspace builds, `pmgctl --selftest` prints
> `PASS`, and the design is written down. What happens next, and in what order,
> is in **[docs/ROADMAP.md](docs/ROADMAP.md)**. What is actually real today is in
> **[docs/STATE.md](docs/STATE.md)**.

---

## The shape of it

```
  Browser (Configure · Design · Edit · Show)
        │  React + the renderer compiled to WASM, so the design canvas
        │  and the playout node run the same layout and text code
        ▼
  Control server  :8519 ── SSE topics (snapshots) ──► browsers
        │                  cue log tail + ?since=<seq> for gaps
        │  PMG Link (CBOR over WebSocket, node dials in)
        │  CAS assets over plain HTTP, Range + resume
        ▼
  Playout Node  :8520  ─── one render thread per output group
        │  named layers: Bug · Lower Third · Ticker · Fullscreen
        ▼
  Outputs (1–4, phase-locked)  →  SDI fill+key · NDI · GPU display
```

Two rules do most of the architectural work:

- **The node is the authority on what is on air. The server is the authority on
  content and assignment.** A server restart must not change a pixel.
- **Snapshots stream; cues are acked.** Anything where a superseded value is a
  *lost event* never rides a conflating transport.

## Where to start reading

| Document | What it answers |
|---|---|
| [docs/VISION.md](docs/VISION.md) | Why this exists, and what it refuses to be |
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | The mental model, the crate map, the invariants |
| [docs/THEORY-OF-OPERATIONS.md](docs/THEORY-OF-OPERATIONS.md) | How a graphic actually gets on air, hop by hop |
| [docs/ROADMAP.md](docs/ROADMAP.md) | M0–M6, what ships when, what is deferred |
| [docs/RISKS.md](docs/RISKS.md) | What might not work, and the spike that will tell us |
| [docs/decisions/](docs/decisions/) | 15 ADRs — every structural choice and its reasoning |
| [docs/specs/](docs/specs/) | The normative formats and protocols |
| [AGENTS.md](AGENTS.md) | Build, test, conventions, and the gotchas list |

## Build

Requires the pinned Rust toolchain (`rust-toolchain.toml`) and Node 20.

```bash
cargo build --workspace
cargo run -p pmg-cli -- --selftest     # must print PASS
npm ci && npm run build && npm test
npm run dev                            # server on :8519
```

## License

Private, all rights reserved. See [LICENSE](LICENSE) — the deviation from the
rest of the ProdMesh family is deliberate and explained there.
