# UI text

The vocabulary is enforceable copy, not a preference. Half of the North Star is
"the operator simplicity of ProPresenter," and a volunteer's experience of
simplicity is mostly the words on the screen.

## The nouns

| Word | Means | Never call it |
|---|---|---|
| **Template** | A designed layout with fields, ready for data | Scene, graphic, page, CG |
| **Item** | Data bound to a Template, with a destination | Take item, instance, element, event |
| **Playlist** | An ordered list of Items | Bin, stack, folder, group |
| **Rundown** | Playlists composed into a show order | Show, sequence, timeline, ROS |
| **Layer** | A named slot on an output holding one live graphic | Framebuffer, channel, bus, engine |
| **Node** | A machine that renders and outputs | Engine, renderer, client, endpoint |
| **Output** | One video output on a node | Channel, port, feed, destination |

## The verbs

| Word | Means |
|---|---|
| **Take** | Put it on air, playing the In animation |
| **Take out** | Remove it, playing the Out animation |
| **Arm** | Prepare it, ready to take. Nothing appears |
| **Update** | Change the data of something already on air |
| **Clear** | Remove immediately. "Clear layer" / "Clear all" |
| **Publish** | Make a Template revision available for use |

**Take, not "fire," "trigger," "play," or "go."** One verb, everywhere, including
in Companion button labels and the API.

## Words that must not appear in the UI

These are all real and correct internally. None of them is a user's problem:

`scene graph` · `SceneDoc` · `SceneInstance` · `cue` (as a noun the user sees —
the *log* is a "Take log") · `epoch` · `seq` · `blake3` · `CAS` · `content hash` ·
`assignment` · `generation` · `premultiplied` · `keyframe track` · `box tree` ·
`WASM` · `compile`

The user publishes a Template; they do not compile a SceneDoc. The user sees "12
files still copying to Booth-1"; they do not see "CAS staging, 12 hashes
outstanding."

## Where broadcast vocabulary *is* correct

**In the control API and its documentation**, where the audience is an integrator
who already speaks it. The API may carry `take_id` on Items, document `Scene` as a
synonym for Template, and use `take` / `take_out` as verbs. Volunteers get plain
words; integrators get familiar ones. Neither audience is served by the other's
vocabulary. (ADR 0015.)

## How things fail, in words

The bar: **an operator reading the message knows what to do next.** No stack
traces, no error codes as the primary content, no "an error occurred."

| Situation | Say |
|---|---|
| Asset still transferring | "Not ready — 2 files still copying to Booth-1 (34 MB left)" |
| Node offline | "Booth-1 is offline. Anything already on air is still on air." |
| Cue not acked | "Booth-1 didn't confirm the take. Check the layer before taking again." |
| Template too new for a node | "Booth-1 is running an older version and can't read this template. Update Booth-1." |
| Genlock lost | "Booth-1 lost reference. Outputs are free-running." |
| Nothing on that layer | "Nothing on Lower Third." |

Prefer naming the thing over naming the failure. "Booth-1 is offline" beats
"Connection error."

## Copy rules

- **Terse labels.** "Take", not "Take on air". Use a `HelpTip` for detail.
- **No intro paragraphs.** A screen starts with the thing it is for.
- **Sentence case** for labels and buttons. Title Case only for proper names.
- **Numbers with units, always.** "800 ms", "1080p59.94", "34 MB left".
- **Say what is on air in the present tense.** "Lower Third · Pastor Dan" — not
  "Item 4 active."
- **A display on a wall never shows "Loading…" or an error.** It shows the last
  known good state, or nothing. Inherited from ProdMesh-Control's `DisplayView`,
  and the reasoning is the same: a screen showing a stack trace is worse than a
  screen showing nothing.

## One register, and the one that was rejected

A design exploration (2026-08-16) proposed a **Simple / Advanced** switch: a
second, gentler vocabulary for volunteers alongside the broadcast one. *Put on
screen* for **Take**. *Name strip, one line* for *L3 Single Line*. *Song words*
for *Lyric Overlay*. It was well written and it is not being built.

The reasoning is worth keeping, because it will be proposed again. The
simplified copy reads better in isolation and costs more everywhere else: two
words for one action means training material that has to teach both, a support
conversation where nobody is sure which screen the other person is looking at,
and a vocabulary that can drift in one register without anyone noticing in the
other. This file is normative precisely so there is one name per thing.

**The plain words here already are the simple register.** Take, Take out, Clear,
Layer, Item — that is the whole point of §1 and §2, and it is what the North Star
means by ProPresenter's simplicity. If a term feels too technical for a
volunteer, the fix is to change it **here**, once, for everybody — not to add a
second word beside it.

What the exploration got right, and is kept: the *supporting* sentences may vary
in detail. "Nothing on this layer" and "Layer 2 · CLEAR" say the same thing at
different densities. Density is a display decision. **Naming is not.**
