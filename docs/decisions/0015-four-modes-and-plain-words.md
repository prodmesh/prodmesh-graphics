# ADR 0015 — Four modes and the plain-words vocabulary

Status: accepted · 2026-08-14

## Context

Vocabulary gets re-litigated by everyone who joins a project, which is the main
argument for writing it down once rather than settling it repeatedly in review.

Broadcast graphics has two established vocabularies and both are genuinely good
at what they are for. Xpression says Scene, Take Item, Take ID, Sequence,
Framebuffer — and "Take ID" in particular is the lingua franca of external
control; any integrator knows what it means. Viz says Scene, Control Object,
Playlist, Show, Channel, with the strongest data-binding vocabulary of the two.

Both are also completely opaque to a volunteer whose only reference is
ProPresenter. "Take ID 3 into framebuffer 2" is not a sentence someone learns on
their first Sunday, and the North Star's first half is exactly that person.

There is also a collision worth avoiding: Viz's "Show" is already ProdMesh-Control's
show-session concept, and reusing it across the two apps would be actively
confusing.

The second half of this ADR is structural. The system does four different things
for four different mindsets — setting up hardware, designing, preparing content,
and running a service — and mixing them produces the interface that most
broadcast graphics systems actually have, where a nervous operator is one wrong
click from editing a template.

## Decision

### Plain words in the UI, broadcast words in the API

| Concept | UI, docs, and user-facing text | API also accepts / documents |
|---|---|---|
| A designed layout | **Template** | `Scene` as a documented synonym |
| Data bound to a Template + destination | **Item** | `take_id` as a stable external identifier |
| An ordered list of Items | **Playlist** | — |
| Playlists composed into a show order | **Rundown** | — |
| A named slot on an output | **Layer** | by name, never by number |
| Put it on air / remove it | **Take** / **Take out** | `take` / `take_out` |

Volunteers get plain words; integrators get familiar ones. Neither audience is
served by the other's vocabulary, and the mapping is documented in one place.

**"Take", not "fire", "trigger", "play", or "go."** One verb, everywhere,
including Companion button labels.

`docs/UI_TEXT.md` holds the enforceable version, including the list of internally
correct words that must never appear on screen — `scene graph`, `SceneDoc`,
`epoch`, `seq`, `blake3`, `CAS`, `assignment`, `premultiplied`, `WASM`, and the
rest. The user publishes a Template; they do not compile a SceneDoc. The user
sees "12 files still copying to Booth-1"; they never see "CAS staging, 12 hashes
outstanding."

### Four modes

| Mode | For | Contains |
|---|---|---|
| **Configure** | Whoever set up the room | Node discovery and adoption, outputs, formats, colour, key mode, layer stacks, reference status, **test patterns**, network and API settings, users |
| **Design** | The person who makes it look right | Template authoring: box tree, text, shapes, media, fields, timelines, sample-data preview, publish |
| **Edit** | The person preparing this week's service | Items, data, Playlists, Rundown order, destinations, readiness |
| **Show** | The person running it | Preview and program, arm, take, take out, clear, the take log, node health |

The modes are separated by *mindset*, not by permission — though they map cleanly
onto ADR 0013's permissions, which is a sign the seams are in the right places.

**Show is the mode that must be boring.** Large targets, no destructive action
within reach of a mis-tap, no way to reach Design from it by accident, and state
displayed in the present tense: "Lower Third · Pastor Dan", not "Item 4 active."

The operator's controls in Show change with the Playlist — that is the
extensibility the system needs — but the *frame* does not: preview, program,
layers, and the log stay where they are, always.

## Consequences

- A volunteer who knows ProPresenter can read the interface without training.
- An integrator reading `docs/specs/http-api.md` finds vocabulary they already
  know, with an explicit mapping.
- Vocabulary arguments have an answer and a place to record a change.
- The four modes give the navigation and the permission model a shared shape.
- **Two vocabularies means a mapping to maintain**, and a real risk that
  broadcast words leak into the UI through an API-shaped feature. `docs/UI_TEXT.md`
  is the check, and it is a review item.
- "Rundown" is the one word here a ProPresenter-only volunteer will not know. It
  is chosen anyway: it is what the thing is called everywhere else in production,
  it is one word to learn, and the alternatives collide with existing concepts.
- Keeping Show boring means deliberately leaving capability out of it, and
  someone will ask for a shortcut into Design from the operator screen.

## Alternatives rejected

**Xpression vocabulary throughout.** Instantly familiar to anyone who has run an
Xpression, and "Take ID" is genuinely the standard for external control. Rejected
for the UI on volunteer legibility; adopted in the API, where that audience is.

**Viz vocabulary throughout.** Same legibility problem, plus "Show" collides with
ProdMesh-Control's show sessions.

**Plain words everywhere, including the API.** Consistent and simpler to
maintain. Rejected because an integrator writing a Companion module or a
switcher macro benefits from the words their other equipment uses, and the cost
of an alias table is small.

**Three modes, merging Edit and Show.** Fewer places to go, and it is what a small
church might actually want. Rejected: editing a rundown and running it are
different mindsets with different risk, and the mode boundary is what keeps a
mis-tap during a service from being destructive.

## Seam

The alias table — `Scene` for Template, `take_id` on Items — is the seam for
integrators and for anyone migrating from an existing system. A future importer
for Xpression or Viz projects has a vocabulary mapping already written down.

The mode split maps onto permissions, which maps onto how a productized version
would license or scope functionality without re-cutting the UI.
