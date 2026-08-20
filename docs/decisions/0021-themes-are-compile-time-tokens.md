# ADR 0021 — Themes are compile-time tokens, and a theme change is a guided republish

Status: accepted · 2026-08-20

## Context

The M5 roadmap names "template variants and themes" without defining either.
The workflows they serve are real and distinct:

- **Themes.** A sermon series rebrand, a Christmas look, a second venue. Today
  that means editing every template's colours and fonts by hand, template by
  template. The design-token idea — `brand.primary` defined once, referenced
  everywhere — is the fix, and the question is where the reference resolves.
- **Variants.** The same words go out as a lower third, a full screen, and a
  wall look (ADR 0018 routes them). Nothing ties those templates together at
  AUTHORING time: they agree on fields by convention, and a field added to one
  silently strands the other two. That half is scoped separately (a manifest
  `family`/`variant` contract the server can enforce, because fields live in
  the manifest) and is not decided here.

The decision that shapes everything is **when a theme token resolves**.

## Decision

**Tokens resolve at compile, in Rust, and the published document carries the
resolved value with the token as provenance.**

```jsonc
"fill": { "rgba": [0.13, 0.45, 0.82, 1.0], "token": "brand.primary" }
```

The `rgba` is real — it is what every reader renders. The `token` records where
it came from, so recompiling under a different theme rewrites the value. The
same pattern applies to gradient stops, outline and shadow colours, and to
`FontSpec` (family/weight from a font token). This is the gradient/fill
fallback pattern from format 1.1 applied again: readers that do not know the
key ignore it, `min_reader` stays 1, and the addition is format **1.2** through
the migration chain.

**A theme is a content-addressed document.** `{ name, tokens: { "brand.primary":
{ color }, "brand.font": { family, weight }, … } }`, stored in the CAS with a
Rust-emitted sidecar, addressed by hash like everything else (ADR 0006). The
server never interprets it; the compiler does. A template that uses tokens
records the theme's hash in its manifest, so "which theme was this compiled
under" has one answer.

**A theme change is a guided republish, not a runtime switch.** Editing a theme
does nothing on its own. "Apply" recompiles each template that references the
theme (client-side wasm — the same compile publish already runs), publishes the
new revisions, and moves Items forward through `update-items` — with the M5v
diff and per-Item verdicts. Anything on air keeps playing the revision it was
taken with until its next take.

## The alternative, and why it loses

Arm-time resolution — the theme rides in the assignment and tokens resolve on
the node — makes a theme change instant and free. It also makes `template_rev`
stop naming its pixels: the same revision renders differently under different
themes, the designer's canvas stops being the output unless the theme is
threaded through every preview path, goldens stop pinning anything, and the
diff in the revision UI can no longer tell an operator what an update changes.
Every guarantee this system leans on runs through "the rev identifies the
render". Instant theming is not worth breaking that, and the republish sweep is
not actually slow: it is one button, and the machinery it drives already
exists and already reports refusals by name.

## Consequences

- Recompiling under a new theme produces **new revisions of every affected
  template**. That is the honest cost, and it is what keeps pinning true.
- The resolver lives in `pmg-scene` (pure set). The designer needs token
  pickers beside its colour and font controls, and a way to choose the theme a
  template follows.
- v1 tokens are **colours and fonts**. Token-driven assets (logos) are
  deliberately later: they drag asset staging into the sweep.
- A template with a token its theme does not define is a **compile error**,
  not a fallback — the value exists in the document either way, but a dangling
  token is a typo worth catching at publish.

## Seam

What this must not foreclose:

- **Per-destination themes.** A wall may one day want the same template under a
  different theme than the key feed. Nothing here prevents publishing the same
  source document under two themes as two revisions and routing them with
  ADR 0018 destinations — that already works. A `variant` mechanism may later
  automate it.
- **Token-driven assets.** The token map is `string → value`; adding an asset
  value kind is additive (format minor bump) and slots into the same resolver.
- **Theme inheritance** (venue theme overriding a base). The theme document is
  self-contained today; a later `extends` field can compose them at edit time
  without the compiler learning anything — the stored theme stays flat.
- **Style variants** (home/away). Most cases are theme swaps once themes
  exist; if a true per-Item variant is still needed after rungs 1–2 ship, it
  becomes an Item-level choice OVER the family mechanism, not a new one.
