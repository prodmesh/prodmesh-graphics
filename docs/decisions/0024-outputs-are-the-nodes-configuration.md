# ADR 0024 — A node's outputs are its own configuration, and the server edits them

Status: accepted · 2026-08-21

## Context

A node's outputs are command-line flags:

```bash
pmg-node --output id=lt,backend=ndi,name=CH\ Lower\ Third,layers=Graphics,audio=2 \
         --output id=sdi,backend=decklink,device=1,key=3,mode=1080p59.94,group=ref
```

The control server can edit an output's **layers** and **colour**, and nothing
else. Changing NDI to DeckLink, adding a display output, assigning a card, or
pairing a key port means editing a shell command and restarting the process.

That is not a configuration story a building can live with, and M6 is the
milestone that says so: an installer implies a service somebody configures, not
a command line somebody remembers. The discovery half is already built —
`OutputCapability` reports every device, its modes, whether it does key+fill,
its paired port, its genlock and its audio pairs — so a node can already say
what it has. There is nowhere to say what you want.

## Decision

**Outputs are the node's configuration, persisted on the node, edited by the
server.**

### The node owns them, because it has to

ADR 0009's autonomy invariant decides this. A node that boots with the server
down must still drive its outputs; a configuration that lives only on the
server is a node that comes up dark after a power cut on a morning the server
is slow. So the list lives in the node's state directory beside `node_id` and
`assignment.cbor`, and the node reads it at boot without asking anyone.

Command-line `--output` flags become the **seed** for a node that has no file
yet — first run, a fresh install, a developer driving a slice — and are ignored
once one exists. A flag that silently overrode saved configuration every boot
would make the saved configuration a lie.

### Hardware and content are configured separately

- **The node's file owns HARDWARE**: which backend, which device, the key
  device for a fill+key pair, the display mode, audio channels, the preview
  port, and which timing group the output belongs to.
- **The assignment continues to own LAYERS and COLOUR**, exactly as today.

Two concerns, two owners, no overlap. Layers are content-facing — "take to
Lower Third" names one — and colour is a property of the signal chain the
server already configures. Moving them would be churn for its own sake, and
having both places able to write either is how they come to disagree.

### Applying is live, and it interrupts that output

Changing an output's hardware means closing a device and opening it again: the
render group is torn down and respawned. The node does this per GROUP, so
reconfiguring the wall does not interrupt the key feed sharing the same
process.

It does interrupt the output being changed, for as long as a device takes to
open. That is unavoidable and it must not be a surprise:

- **A reconfiguration of an output with something ON AIR is refused**, and the
  refusal names the graphic. An operator who means it sends `force`.
- The node **acknowledges what it actually applied**, not what it was asked
  for. If a device fails to open, the node says so and keeps the configuration
  that was working — a node that accepts a broken configuration and goes dark is
  the failure this whole path exists to avoid.

### It travels as its own message, not in the assignment

The assignment is this week's content, rebuilt whenever an Item changes.
Carrying plant configuration in it would mean rebuilding output backends
because somebody fixed a typo in a lower third. `SetOutputs` is its own message
with its own ack, and the node persists on receipt — the same
persist-then-ack ordering assignments already use (ADR 0009).

## The alternative, and why it loses

**Outputs in the assignment.** Fewer messages, and the node already persists
assignments so autonomy would survive. It loses because content and plant then
share a lifecycle: every Item edit re-sends the output configuration, and a bug
in content handling can take the outputs down. They change on completely
different timescales, by different people, for different reasons.

**Server-owned, node-obedient.** Simplest to reason about and it breaks
autonomy: a node cannot come up correctly without the server, which is the one
promise this system makes about a bad morning.

**Stage it and apply on restart.** No interruption, and it is honest only where
there is a way to restart a node. There is no installer yet, so "restart the
service" means finding the terminal window someone launched it from — which is
exactly the workflow this ADR exists to end.

## Consequences

- The node's runtime learns to stop and respawn one render group. It already
  has a per-run stop flag; this makes it per group, which is a small change with
  a real failure mode — a device that will not reopen — that has to be handled
  rather than assumed away.
- The file is **JSON, not CBOR** — unlike the assignment beside it. A node's
  assignment is machine content; this is a small file a person may have to read,
  or fix, over SSH on a morning when the browser is not the answer.
- Configure gains the editor: backend, device from the node's own capability
  report, mode, key pairing, audio, group.
- The seeding rule means an existing deployment keeps working: the first boot
  after this lands writes the flags it was started with into the file.

## Seam

- **Output profiles** — "1080p59.94 fill+key on the Quad 2" as a named thing
  applied to several nodes. This is a per-node file today; a profile is a
  server-side template that writes one.
- **Hot-plug.** Capabilities are reported at connect. A card appearing while the
  node runs needs a capability refresh, which is additive to the same message.
- **Audio routing** — which internal playback channels reach which output
  pairs. It belongs in this file, beside `audio_channels`, and is deliberately
  not designed here.
- **`pmgctl node outputs`** — printing and setting the configuration from a
  terminal. The file being readable JSON covers the emergency; a command that
  validates before writing is the next step and is not built.
