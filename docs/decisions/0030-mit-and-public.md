# ADR 0030 — MIT-licensed and public, like the rest of the family

Status: accepted · 2026-10-10 · supersedes the licensing posture in `docs/VISION.md`

## Context

Since its first commit this repo has been private and all rights reserved, the
one deliberate deviation from a family that is otherwise MIT. `docs/VISION.md`
recorded why: "in-house now, product later", and *choosing a permissive license
later is easy; un-choosing one is impossible.* The `LICENSE` file said any
change to it belongs in an ADR. This is that ADR.

The premise underneath "product later" was that a desktop application's value
is protected by keeping its source closed. That premise has stopped holding.
Over the autumn of 2026, shipped desktop software and games have been routinely
decompiled, reverse-engineered, rebuilt and re-released with model assistance.
A closed binary of a playout node is not a moat; it is a delay. A private repo
gives up everything an open one offers — other booths running it, people who
find the bugs we cannot (D3D12, Vulkan, cards we do not own), a reference other
people can learn from — in exchange for protection that no longer exists.

The repository on GitHub (`prodmesh/prodmesh-graphics`) has been made public.
A public repo carrying a proprietary `LICENSE` would be the worst of both: the
code visible to everyone and usable by no one in good faith.

## Decision

**ProdMesh Graphics is MIT-licensed, the same as ProdMesh-Control,
prodmesh-caption and prodmesh-rta.** `LICENSE` is the standard MIT text;
`Cargo.toml` and `package.json` say `MIT`. The project continues in the
family's spirit: open, experimental, and offered in goodwill.

**The `## Seam` convention stays, and CI keeps checking for it.** Its question
changes from "what must this not foreclose if we sell it?" to "what must this
not foreclose for somebody else who deploys it?" — which turns out to be nearly
the same list. A church across town is a customer in every sense that mattered
to the seams: their templates must open in two years, their fonts are their
licensing problem, their NDI runtime is theirs to install.

**Vendor SDKs still never enter the repo**, and being public raises the stakes
rather than lowering them. DeckLink and NDI are licensed by their owners, not by
us, and our MIT grant cannot extend to them. `sys/*/vendor/` stays gitignored,
the hand-written shim headers remain the only vendor-shaped code here, and the
NDI runtime is still loaded at run time rather than linked or bundled.
What a signed release binary may contain is governed by each vendor's terms,
which this ADR does not change.

**ffmpeg stays LGPL-only, run as a child process.** That rule was written for a
product; it applies unchanged to an MIT project whose releases anybody may
redistribute.

**"Licensing hooks" leaves M6.** It existed to make a paid edition possible.
There is no paid edition to gate.

## The alternatives, and why they lose

- **Stay all rights reserved, in a public repo.** Lawful, and pointless: it
  forbids exactly the reuse that being public invites, and forbids nothing to
  anyone willing to ignore it.
- **Make the repo private again.** Reverses the decision this ADR records, for a
  protection the context argues is gone.
- **Source-available (PolyForm Shield, BSL, and kin).** Visible but not free to
  compete with. It asks for the goodwill of an open project while withholding
  the thing that earns it.
- **A copyleft licence (GPL, AGPL).** Would stop a fork from being re-closed.
  It would also collide with the proprietary vendor SDKs the node exists to
  talk to — linking a GPL node against the DeckLink API is the kind of question
  that ends in a lawyer — and it breaks the family's single convention for no
  benefit anybody has asked for.
- **Apache-2.0.** A better patent clause, and a reasonable choice in isolation.
  Not worth being the one ProdMesh repo with a different licence.

## Consequences

- **It cannot be undone.** Everything published under MIT stays available under
  MIT. The old `LICENSE` said so in advance; this is the decision it anticipated.
- **Anyone may take the code, rebrand it and sell it.** That is accepted, not
  overlooked. MIT grants no rights to the ProdMesh name or logo, and nothing here
  changes that.
- **Contributions arrive under MIT** by the usual inbound=outbound convention.
  No CLA.
- **The documents speak to strangers now.** README, AGENTS.md and the
  `docs/` set were written for one booth; they are now the first thing an
  outsider reads. "Our campuses" survives where it is the honest origin story,
  and goes where it would leave a newcomer stranded.
- **Seed and rehearsal data must stay fictional.** It already is: the names in
  `tools/rehearsal-service.py` are invented. VISION's "no customer data in the
  design" now also means none in the repository.

## Seam

**Vendor terms stay at the `sys/` boundary.** Being MIT must never be read as
permission to commit an SDK header, bundle the NDI runtime, or link a GPL
library for convenience; each of those would make a release something we, or
anyone redistributing it, may not ship. The trademark question is kept separate
from the copyright one, so the name can be protected later without touching the
licence.
