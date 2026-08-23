# ADR 0029 — Director and Playout are two menu-bar apps, and one launcher

Status: accepted · 2026-08-22

## Context

Everything runs from a shell script. `tools/rehearsal.sh` starts a server and a
node and hands over; that is right for development and is not a thing anybody
outside this building can install. M6's roadmap has carried "an installer, and
documentation for someone outside our building" with no decision about shape.

The family has already answered part of it: `prodmesh-caption` and
`prodmesh-rta` both ship as **macOS `.app` bundles** with `Info.plist.in`, an
entitlements file, and Developer ID signing and notarization from CI. Following
that is cheaper than inventing, and an operator moving between the apps meets
one idea of what a ProdMesh app is.

The two processes here are not one product, though, and pretending they are
would be the mistake. A **control server** owns content and assignment; a
**playout node** owns what is on air, and ADR 0009 says a node that loses the
server keeps playing. They are installed on different machines, they fail
independently, and a booth may have several of one and none of the other.

## Decision

**Two apps: ProdMesh Director (the server) and ProdMesh Playout (the node).**
Separate bundles, separate identifiers, separate data. A machine may have
either, or both.

### Menu-bar items, not windows

`LSUIElement` — no Dock icon, no app switcher entry. **The browser console is
the real interface**, and a second one would be two places that must agree about
what is on air. What the machine itself needs to say is smaller than a window:
is this up, is anything on air, where is the log, how do I open the console.
That is a menu.

The icon is a dot, and it is **red only when something is on air** — ADR 0020's
rule, which matters more here than anywhere else in the product. This icon sits
beside the clock for a whole service; an eye that has learned red means
"problem" will not read it as "live".

### One launcher binary, configured per bundle

Both apps ship the same Swift binary and differ only in `Launcher.json` in
their Resources: the child to run, its arguments and environment, the health
endpoint, what "Open…" opens.

Two launchers with two codebases would drift. One would learn to restart on
crash, or to put its log somewhere findable, and the other would not — and
nobody would notice until the night the other one was needed.

### The app IS the supervisor

Not a LaunchAgent. A LaunchAgent would restart the child too, and it would put
the thing an operator has to reason about in a plist they cannot see, started by
a system they cannot ask. Here: the menu says whether the child is up, the log
is one click away, and quitting the app stops the child — which is what
everybody expects and what a LaunchAgent specifically does not do.

Restarting is not optional. ADR 0009's reasoning one level down: a node that
crashes at 10:15 on a Sunday has to come back without anybody knowing where the
binary lives. With a backoff, capped, because a binary that cannot start at all
must not become a spin loop in a booth. Stopping is SIGTERM then SIGKILL —
quitting must not leave an orphan holding a DeckLink, which the next launch
would meet as a device that will not open, with no explanation anywhere.

**"Start with this Mac" is `SMAppService`**, not an installed plist: one API
call, visible and revocable in System Settings, and nothing left behind when the
app is dragged to the Trash.

### Status comes from the process, not from the fact that it is running

The menu polls the node's own status page and the server's own health endpoint.
A process can be up and not linked, up with no outputs, up and refusing to
render; *"the icon is green because `fork` returned"* is the reassurance that
gets somebody to Sunday without checking.

### Data lives in Application Support, never in the bundle

`~/Library/Application Support/ProdMesh Director` and `…/ProdMesh Playout` — the
database, the CAS, the node's identity, the log. An app that writes to itself
cannot be replaced by dragging a new one in, and on a booth machine the thing
you must never lose is the state, not the binary.

### The Node runtime travels with Director

A booth machine has no nvm and should never be asked to install one. An app that
depends on whichever `node` happens to be on the PATH is an app that breaks when
somebody upgrades something unrelated. It costs about 180 MB and it is the
difference between an install and a set of instructions.

### macOS first

Signed and notarized `.app`s, matching the CI signing that already exists.
Windows follows once the shape is proven on one platform — it needs a service or
tray app, an MSI or NSIS installer, and a signing story that does not exist yet.
Doing both at once would mean twice the packaging work before either is proven.

## The alternatives, and why they lose

**One app that runs both.** Simpler to install and wrong about the product: the
two are installed on different machines and fail independently, and merging them
would make "a node that keeps playing when the server dies" a sentence about two
halves of one process.

**A LaunchDaemon, no UI at all.** Correct for a headless server and wrong for a
node: the renderer needs a logged-in GUI session for Metal and for the DeckLink,
and nothing on screen would say a node is alive when the browser cannot reach it
— which is exactly when somebody needs to know.

**A real window.** More discoverable for a newcomer, and one more UI to build
and keep true, duplicating what Configure already shows. The menu bar is what a
thing that boots with the machine and is never chosen deliberately should be.

**`pkg`/SEA to make Director a single binary.** Attractive, and `better-sqlite3`
is a native addon that makes it awkward. Bundling the runtime is boring and
works.

## Consequences

- `tools/package-macos.sh` builds both, unsigned by default — that is what you
  want fifty times a day — with `--sign` for a release.
- **Found by packaging, and only findable that way:** `server/index.js` decided
  whether it was the entry point by comparing `import.meta.url` with
  `file://${process.argv[1]}`. A path containing a SPACE encodes as `%20` in one
  and not the other, so inside "ProdMesh Director.app" the server started,
  listened to nothing, and exited 0 with no output. It cannot happen in a repo
  checkout; it happens the moment the thing is installed.
- A node names itself by the SHORT hostname now. A managed network answers
  `hostname` with a fully qualified name, and that was what appeared in every
  node picker and every take-log line.
- **Local Network permission is per responsible app**, so a fresh install starts
  denied and mDNS discovery silently finds nothing — indistinguishable from a
  network with no Director on it (`docs/INTEGRATION-NOTES.md`). The menu names
  the setting rather than leaving the log as the only place that says so.

## Seam

- **Windows**: a tray app of the same shape over the same two children. The
  launcher's configuration file is deliberately platform-neutral.
- **A DMG with a background image**, and notarization stapled, for a release
  that is downloaded rather than built.
- **Several nodes on one machine** — the launcher config already carries the
  ports; it needs a second bundle or an instance argument, not a new design.
- **Settings in the menu** — which Director to dial, which outputs to run —
  rather than editing `Launcher.json` in the bundle. Configure already owns
  output hardware (ADR 0024); the server address is the one thing that cannot
  come from the server.
