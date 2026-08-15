# testdata

| Directory | Contents |
|---|---|
| `templates/` | Fixture templates, in the **JSON mirror** of the scene format |
| `golden/` | Rendered PNGs — the native regression corpus and the native↔WASM parity corpus |
| `migrations/` | Frozen fixtures, one per format version step |

## Two rules

**A shipped migration fixture is never edited.** If it is wrong, add a new
migration step. Editing one silently changes what "we could read v1.0" means,
which is the whole guarantee (`docs/specs/scene-format.md` §9.3).

**Updating a golden image is a deliberate act.** The commit that changes one
says in its message what visually changed and why it is correct. A golden that
drifts silently is worse than no golden at all.
