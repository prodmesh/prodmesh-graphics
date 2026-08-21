# Golden images

The three PNGs here are spike S1's **record**: the primitives as they rendered
on Metal, 2026-08-14. Updating one is a deliberate act, and the commit that does
it says what visually changed and why it is correct.

**The template corpus is not committed, and that is on purpose.** `pmgctl
render-golden` renders ~90 real SceneDocs (`pmg_render::corpus`) alongside the
primitives, and none of those images live in git, because a committed baseline
is only meaningful against a reproducible input and a GPU is not one — these
three PNGs are Metal artifacts, and a Linux runner would fail a byte compare
against them for reasons that are not bugs.

So the corpus is compared three ways, all of them against something rendered in
the same conditions:

| Comparison | What it catches | When |
|---|---|---|
| native vs native, twice | non-determinism in our own renderer | every CI run (`parity.yml`) |
| native vs WASM, same font bytes | the S1 bar — do the two builds of one renderer agree? | by hand, recorded in `docs/INTEGRATION-NOTES.md` |
| before vs after a change | an unintended visual change from a shader, uniform, or wgpu edit | locally, `docs/TESTING.md` |

```bash
pmgctl render-golden /tmp/before --font /System/Library/Fonts/Supplemental/Arial.ttf
# ... make the change ...
pmgctl render-golden /tmp/after  --font /System/Library/Fonts/Supplemental/Arial.ttf
pmgctl golden-diff /tmp/before /tmp/after
```

`wasm/` holds raw RGBA posted back by the browser harness, named
`<figure>.<w>x<h>.rgba` — raw bytes carry no dimensions, so the name does.
