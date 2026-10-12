# Vision

## The North Star

**The operator simplicity of ProPresenter, with the authoring stability,
performance, and maturity of Ross Xpression or Vizrt.**

Those two halves are usually sold as a trade. The systems a volunteer can learn
in twenty minutes are the ones that fall over when you ask them to hit a frame.
The systems that never miss are the ones that need a full-time operator and a
week of training. That gap is the whole reason this project exists, and holding
both ends of it is the thing to protect when a decision gets hard.

Concretely, what each half means here:

**ProPresenter simplicity** — a volunteer opens the app, sees a list of things,
clicks one, and it goes on screen. There is no "framebuffer," no "take ID," no
"channel," no scene tree, no compile step they have to know about. The words on
screen are Template, Item, Playlist, Rundown, Layer, Take, Clear. When something
is wrong the app says so in a sentence, before air, not after.

**Xpression/Viz stability** — the graphic goes on at the frame you asked for,
looks the same on every machine, and does not care that the network hiccuped or
that someone opened a browser tab. Templates authored two years ago still open.
A designer's preview matches what the switcher sees. Nothing on air changes
because a server restarted.

## What this is

A distributed system for **templated keyed content**. Data goes into a designed
Template; the result is rendered by a Playout Node to SDI, NDI, or a display
output, usually as fill and key into a switcher.

The four things it does, in the order a user meets them:

- **Configure** — find and set up playout nodes, their outputs, formats, colour
  space, key/fill wiring, layer stacks, and reference. Prove each one with a test
  pattern before anyone trusts it.
- **Design** — build Templates: responsive layouts with text, shapes, images,
  and media, animated by named timelines.
- **Edit** — bind data to Templates as Items, arrange them into Playlists, and
  compose Playlists into a Rundown.
- **Show** — preview, arm, take, take out, clear. From a browser, a Stream Deck,
  a DashBoard panel, a switcher's GPO, or another system's API call.

## What it refuses to be

- **Not a compositor in the program path.** Graphics are keyed downstream. If a
  node dies, the show is missing a lower third, not missing a picture.
- **Not a scripting environment.** Data bindings are a total expression language
  with a fixed set of pure string transforms. Templates are documents, not
  programs. A template must not be able to throw at air time. If a case ever
  genuinely needs a sandboxed evaluator, that gets its own ADR and its own
  failure story — it does not creep in.
- **Not ProPresenter's replacement, yet.** ProPresenter keeps lyrics, IMAG, and
  screens. This is the keyed CG layer. The *data model* is designed so screen
  content could move here later — responsive text, media playback, and
  multi-screen destinations are first-class from day one — but the *renderer
  scope* stays narrow for v1. A CG glitch is embarrassing; a lyrics glitch stops
  worship. Different failure budgets, and mixing them early means the strict one
  sets the pace for everything.
- **Not a cloud service.** It is a LAN appliance for a building, on a network
  with no internet and no TLS. Everything it needs is on local disk before the
  service starts.

## Open, and the seams that keep it deployable

This was built **for our own campuses first**, and it is now **MIT-licensed and
public**, like the rest of the ProdMesh family (ADR 0030). Until October 2026 it
was private and all rights reserved, to keep productization available; ADR 0030
records why that stopped being worth protecting.

What survives from that posture is structural: **every ADR carries a `## Seam`
section** naming what that decision must not foreclose. The question it asks is
now "what must this not foreclose for somebody else who deploys it?" — and the
list barely changed. Those seams, collected:

| Seam | Why it matters to anyone who deploys it |
|---|---|
| **Scene format compatibility** | `format_version` + `min_reader`, unknown keys preserved on read, a frozen migration fixture per version step. Customers' templates must open in two years. |
| **Font licensing** | Embedding font bytes in an export bundle is redistribution. Fine in-house; not fine shipped. One field — `include_fonts: bool` vs reference-by-identity — decided now rather than as a format break later. |
| **ffmpeg licensing** | LGPL components only, dynamically linked, no GPL. Written down so nobody links `libx264` for convenience. |
| **NDI SDK redistribution** | Loaded at runtime, never statically linked, so distribution terms stay a deployment question. |
| **DeckLink SDK** | Header-only usage under Blackmagic's terms; the shim keeps the boundary in one directory. |
| **Protocol versioning** | PMG Link negotiates `proto_ver` at handshake and carries an `at:` field it does not yet use, because adding one later is a break. |
| **Deployment shape** | Server runs from a git checkout, a container, or a desktop launcher — the same three ProdMesh-Control supports. No assumption that we own the box. |
| **No customer data in the design** | Nothing about our campuses, our Planning Center, or our room names is baked into a schema. |

If a decision would close one of these seams and save a week, the week is not
worth it. If it would close one and save a month, that is an ADR with a real
argument in it.

## The measure of success

Not features. Three moments:

1. A volunteer who has only ever used ProPresenter takes a lower third on air
   correctly, on their first Sunday, without being talked through it.
2. A designer changes a template on a laptop, and what the switcher shows is what
   they saw — same fonts, same wrap, same timing.
3. The control server gets restarted mid-service and nobody in the room notices.

The third one is the hardest and the most important, and it is why
`docs/decisions/0009-nodes-are-autonomous.md` exists.
