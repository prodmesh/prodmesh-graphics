# Getting started

From a clean machine to a graphic on air, for someone who has not been in our
booth. If a step here is wrong for you, that is a bug in this page; please open
an issue.

**What has actually been run:** everything below, on Apple silicon Macs. The
playout node is written for macOS, Linux and Windows, and only macOS has been
exercised. Reports from the others are very welcome.

---

## 1. What you need

| | For | Notes |
|---|---|---|
| **Rust** via [rustup](https://rustup.rs) | the renderer, the node, the CLI | The toolchain is pinned in `rust-toolchain.toml`; rustup installs it, and the `wasm32-unknown-unknown` target, on first build. |
| **wasm-bindgen CLI** | building the browser half | Its version must match the crate's exactly: `cargo install wasm-bindgen-cli --version 0.2.127` (the version in `Cargo.lock`). |
| **Node 20+** | the control server and the web app | |
| **A browser with WebGPU** | Design, and the canvas everywhere | Current Chrome or Safari. Design renders with the same Rust the node runs, compiled to WebGPU, so there is no fallback renderer. |
| **ffmpeg and ffprobe** on `PATH` | video and audio clips | Stills, text and shapes work without them. Clips are transcoded to an all-intra format on upload. |
| **NDI runtime** | NDI output | Install the [NDI SDK or runtime](https://ndi.video/for-developers/ndi-sdk/). The node loads it at run time. |
| **Blackmagic Desktop Video** and the **DeckLink SDK** | SDI output | Only for SDI. See below. |

Nothing here reaches the internet at run time. Fonts and assets travel inside
the system, because booth machines usually have no internet.

### The DeckLink SDK

It is licensed by Blackmagic and is never committed here. Download it, unpack
it somewhere that will not be cleaned up, and link it in:

```bash
ln -sfn "/path/to/Blackmagic DeckLink SDK 16.0" sys/decklink-sys/vendor
```

Without it, everything builds and SDI is simply unavailable. With it, keep
Desktop Video (the driver) at least as new as the SDK. An older driver doesn't
fail. It reports default capabilities that look plausible and are wrong;
`docs/INTEGRATION-NOTES.md` has the measured details.

---

## 2. Build, and prove the build

```bash
cargo build --workspace
cargo run -p pmg-cli -- --selftest     # must print PASS
npm ci && npm run build && npm test
```

`--selftest` checks the scene format's load-bearing guarantees. If it does not
print `PASS`, stop there.

---

## 3. See the whole thing in two minutes

```bash
tools/rehearsal.sh --all-ndi
```

This starts a control server and one playout node with three NDI outputs, then
builds a fictional Sunday service: templates, Items, four Playlists and a
Rundown. Open <http://127.0.0.1:8519/show>, click into the second song, and
arrow through it. Watch the outputs in any NDI viewer: *CH Lower Third*,
*CH Full Screen* and *CH LED Wall*.

Stop it with `tools/rehearsal-stop.sh`. Its data lives in `var/rehearsal/`;
`--reseed` starts that over.

---

## 4. Your own setup, from nothing

### Start the control server

```bash
npm run build
npm start                                  # serves the console on :8519
```

It keeps its database and asset store under `var/` unless `PMG_DB` and
`PMG_CAS` say otherwise.

### Start a playout node

On the same machine or another one on the network:

```bash
cargo build --release -p pmg-node
./target/release/pmg-node --name booth-1 \
  --output "id=lt,backend=ndi,name=Lower Third,layers=Graphics"
```

With no `--server`, the node looks for the server over mDNS. If they are on
different subnets, or multicast is filtered, give it `--server <host>:8519`.
Manual entry always works. On macOS the first mDNS attempt may need
**System Settings → Privacy & Security → Local Network** to allow the terminal
or the app.

For SDI, find your connectors first, then name them:

```bash
cargo run -p pmg-cli -- decklink list
./target/release/pmg-node --name booth-1 \
  --output "id=lt,backend=decklink,device=1,key=3,mode=1080p59.94,layers=Graphics"
```

`device` is the fill connector and `key` the key connector of a fill+key pair.
Leave `key` out for fill only. `pmg-node --help` lists every option.

### Adopt the node

Open <http://127.0.0.1:8519/configure>. The node appears as unadopted, and an
unadopted node is given nothing at all. Click **Adopt**. Its outputs, layer
stacks and colour settings are now editable here, and they are pushed to the
node, which stores them itself.

### Design a template

In **Design**:

1. Open **Media Browser** and **Add a font…**. A template renders only with
   font files the server holds. That is deliberate: it is how a designer's
   laptop and the booth render the same letters. Add the italic and bold
   files too if you will use them; they are separate files.
2. **New template…**, give it a name, then **Add text**. In the inspector,
   pick the font family, size, and **Italic** if you want it.
3. Type `${name}` into the text binding. A `${field}` becomes a field every
   Item of this template fills in.
4. Add timelines named **In** and **Out** for how it arrives and leaves, then
   press space to play one.
5. **Publish**. Items always point at a published revision, so later edits
   never change what is already on air.

### Make Items, and a Rundown

In **Edit**: **New playlist**, then **New item**. Choose the template, fill in
its fields, and choose where it goes: a node, an output and a layer, or a
named screen. A playlist is either **Cue** (arrow to arm, then take) or
**Follow** (navigating takes, for song lyrics). **New rundown** puts
playlists in running order.

### Take it

In **Show**, select an Item and press **Take next**, or use the arrow keys.
The button turns green only when the node confirms the graphic is on air, and
a take that would land too late is refused rather than fired.

---

## 5. Before a real service

- **Secure the server.** It ships open, and it says so in the footer. The
  first user you create in Configure becomes the administrator and turns
  enforcement on. If you lose that account, `npm run grant-admin` on the server
  machine restores it.
- **Sizing.** One node renders 1–4 outputs; outputs that share a reference
  clock share a render thread (`group=` in `--output`).
- **The node keeps playing if the server goes away.** That is the design, not
  luck. Restarting the server mid-service should not change a pixel.
- **SDI colour.** By default the card converts RGB to YCbCr with its own
  settings. **Encode YCbCr on the node** (Configure, per DeckLink output) applies
  your matrix and ranges instead. It has not yet been run on a card, so try it
  in a rehearsal before a service.
- **Control surfaces.** OSC on 8522, RossTalk on 8524, and an HTTP API on the
  node itself (8520) that still works with the server down. See
  `docs/specs/control-surfaces.md` and `docs/specs/openapi.json`.

## 6. The macOS apps

`tools/package-macos.sh` builds **ProdMesh Director** (the server) and
**ProdMesh Playout** (the node) as menu-bar apps, unsigned by default. Signed
and notarized builds are not published yet.

## Ports

| Port | What |
|---|---|
| 8519 | Control server: the console, the API, PMG Link |
| 8520 | Node API and status page |
| 8521 | Node preview stream |
| 8522 | OSC in (UDP) |
| 8524 | RossTalk in (TCP) |

## When something is wrong

- `docs/STATE.md` says what is real and what is not, so check it before assuming
  a feature exists.
- `docs/INTEGRATION-NOTES.md` covers vendor SDK and hardware behaviour that
  looks like a bug and isn't, each entry dated and measured.
- `AGENTS.md` has the gotchas list, which is worth reading before changing
  anything.
