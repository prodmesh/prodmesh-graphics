# ADR 0020 — The operator console design language

Status: accepted · 2026-08-18

## Context

The web app had a functional four-mode shell in ProdMesh-Control's visual
idiom: a left sidebar, the dashboard palette, page headings, forms. It worked
and it looked like what it was — a management dashboard. The product is not a
dashboard. It is a live playout console, and a design exploration (Figma +
Claude Design, 2026-08-16 → 08-18) produced a coherent language for it:
1440×900 console frames for all four modes, a token set, a 33-component
inventory, and behavioural invariants that agree with ADRs 0006/0008/0017/0018
almost verbatim.

This ADR adopts that language and records where the exploration was overruled
by decisions this repo had already made — because a design document arriving
after the ADRs it contradicts does not reopen them.

## Decision

**Adopt the ProdMesh GFX operator-console design system** as the web app's
visual language, transcribed into `src/styles/tokens.css` (as `--pm-*` custom
properties) and `src/components/console.tsx`.

The load-bearing rules, normative for all feature UI:

- **A near-black neutral ramp and exactly three signal colours.** Red
  `#FF3B30` means ON AIR and only ever ON AIR; cyan `#00F0FF` means armed /
  cursor / next; green `#30D158` means healthy. There is no fourth signal.
  A failure is a **sentence**, not a colour — red on an error would teach the
  eye that red sometimes doesn't mean "on air".
- **Flat surfaces, 1px inset hairlines.** One drop shadow exists in the whole
  product: under the Take button. The heaviest stroke is the 1.5px ring on an
  on-air tile. Panels have no radius; they are chrome, not cards.
- **Fixed chrome, flexing centre.** Header 48, toolbar 44, footer 36; rails
  320/280, inspectors 360, scrolling internally. Nothing floats: no modals,
  popovers, toasts or drawers. "Everything visible at a glance" is the premise.
- **Three type families, each with a job.** Inter for chrome; Spline Sans Mono
  for anything measured, identified, routed or timestamped; Instrument Serif
  only inside rendered broadcast output, never in chrome. ALL CAPS runs carry
  1px tracking and are reserved for state and structure (`ON AIR`,
  `LAYER 2: OVERLAY`); buttons stay sentence case per `docs/UI_TEXT.md`.
- **Chrome does not animate state.** An operator glancing at the screen must
  read the current state in the first frame. 120ms opacity is the ceiling;
  a button easing to a confirmed colour before the node confirms is the
  specific failure this product exists to prevent.
- **Numbers are measured, never decorative.** The telemetry chip renders the
  node's reported fps with its trailing zeros, or does not render. No metric
  is invented to fill a card.
- **The shell is a top header with mode tabs**, not the family sidebar. This
  deviates from "identical tokens to ProdMesh-Control" deliberately: an
  operator console and a management dashboard are different rooms, and the
  console's states (armed/air discipline) must not inherit a palette in which
  red sometimes means "error".

### Where the exploration was overruled

Each of these appears in the imported design and is **not** built, because an
accepted decision already answers it:

1. **The Simple/Advanced register switch.** `docs/UI_TEXT.md` §"One register"
   rejected it on 2026-08-16: one name per thing, for everybody. What survives
   is the narrow reading — supporting sentences may vary in density; naming
   may not.
2. **Cue/Follow as a console toggle.** ADR 0017: mode is a property of the
   Playlist, authored in Edit. Show renders a persistent **indicator** and the
   keyboard map follows the focused playlist; a toggle on the operator's screen
   is exactly the "depends who logged in last" failure the ADR rejected.
3. **Optimistic Follow.** The exploration set slides on air locally. ADR 0017
   rejects this outright; the cursor moves locally and instantly, the on-air
   mark waits for the node's ack, in both modes, always.
4. **Per-destination field overrides** (the OVERRIDE/SHARED toggles in the
   Item inspector). ADR 0018 rejected them as scope: they reopen "which one is
   the real Item". The Item carries one set of data; per-destination templates
   are how the same words look different per feed.
5. **CDN fonts and CDN icons.** The design system loads Inter, Spline Sans
   Mono and Instrument Serif from Google Fonts and suggests unpkg for missing
   Lucide glyphs. No CDNs, ever — the fonts ship in the bundle via
   `@fontsource`, and icons come from the already-bundled `lucide-react`.
6. **Fake telemetry and version strings** (`V2.4.1-GPU`, `60.0 FPS`,
   `Link Quality: 99.8%` as static copy). The header shows `/api/version` and
   the fps the node actually reports; readouts render real state or nothing.
7. **"Console Playout History".** The log is the **Take log**
   (`docs/UI_TEXT.md`: `cue` never appears as a user-facing noun).
8. **The timed Show** (program clock, auto-clear at 0:00, countdown rules).
   Nothing behind it exists yet — timers are a P1 input type scheduled for M6,
   and an auto-clear is a node-side rule, not a browser animation. Deferred,
   not dropped: the design's treatment (the node clears it; the console
   reports it in the log) is the right shape when M6 arrives.

### What was adopted beyond the pixels

- The **armed inspector** pattern: the values about to be sent sit next to the
  button that sends them, with per-destination readiness and the reasons for
  any exclusion named before the press.
- The **stacked destination rows** in Edit's item list (the exploration's own
  recommendation): every destination readable without opening the Item,
  because a count hides exactly the destination that is excluded.
- The **slide tile grid** with a **section jump map** for the cursor playlist
  in Show — the Canyon Hills "sing the chorus again" case as a row of targets
  rather than a list to walk.
- **Clear all is press-twice.** No overlay to confirm with, and a mis-tap must
  not empty every layer; the primed state disarms itself after 3 s.

## Consequences

- `src/styles/tokens.css` no longer matches ProdMesh-Control's. The family
  rule survives as a principle (tokens in one file, no hex in feature CSS) but
  the palette is the console's own. An operator moving between the two apps
  now feels a room change — that is intended.
- The component library is first-party TSX + CSS (`components/console.tsx`,
  `styles/console.css`), still no third-party component library.
- Legacy classes survive only in Configure (`styles/shell.css`, trimmed) and
  are deleted as it migrates.
- Old Show/Edit page layouts are gone; their behaviours (take never lies,
  readiness gates arming, two marks, mode refusal, gap-recovering take log)
  carried over and their tests were rewritten to the new structure with the
  claims intact.

## Alternatives rejected

**Adopt the design verbatim, contradictions included.** A design that toggles
playout mode in Show or renders "60.0 FPS" as copy is wrong per ADRs that were
argued and accepted; importing it wholesale would relitigate them by stealth.

**Keep the dashboard idiom and restyle incrementally.** The exploration's value
is exactly its system — the three-signal discipline and the fixed-chrome frame.
Half of it grafted onto a sidebar dashboard keeps neither promise.

**Rebuild the chrome in WASM while repainting anyway.** Rejected by ADR 0019
the same week: the canvas and its geometry are Rust; the chrome's text input,
focus and accessibility are the browser's, and React keeps them.

## Seam

The tokens are one file and the components are presentation-only — nothing in
`components/console.tsx` fetches, subscribes, or knows a route. That is what
lets a future surface (a tablet layout, a hardware panel's confidence display,
a per-venue theme) reuse the language without importing the app; and it is what
keeps a later Configure migration, or the M6 timed-Show work, an addition
rather than a rework.

Related: ADR 0015 (four modes and plain words), ADR 0017 (playout mode per
playlist), ADR 0018 (one Item, several destinations), ADR 0019 (React keeps the
chrome; the canvas and its geometry are Rust).
