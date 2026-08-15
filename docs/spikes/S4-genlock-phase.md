# S4 — Genlock and phase across machines

**Gates:** M3 · **Risk:** [R2](../RISKS.md#r2) · **ADR:** 0011
**Status:** not started

## Question

Two machines on the same reference: how well do their outputs stay in phase, and —
the question this spike really exists to answer — **how far apart do their "take
now" moments actually land?**

The second number is the point. It quantifies what v1 explicitly does *not*
promise, so that the promise in ADR 0011 is backed by a measurement instead of an
assertion.

## Method

1. Two machines, each with a DeckLink, both fed the same blackburst or tri-level
   from one generator via a DA.
2. Each renders a **frame counter burned into the picture**, plus a moving 1-px
   line.
3. Capture both outputs side by side — a switcher's multiviewer recorded, or two
   capture inputs on a third machine.
4. **Phase:** measure drift over one hour. Confirm lock is held and detect any
   slip.
5. **Take alignment:** fire the same take on both nodes as simultaneously as the
   control path allows, 100 times. Measure the distribution of the frame offset
   between them.
6. Repeat 5 with the nodes' local surfaces (GPI wired to both) rather than the
   server, to separate protocol latency from everything else.
7. Then the negative control: **pull reference from one machine** and measure how
   long until `node:<id>:sync` reports loss.

## Success bar

| Metric | Bar |
|---|---|
| Phase over one hour, both on reference | No slip; outputs stay frame-aligned |
| Fill+key on one device | Phase-identical — this is the promise that must hold |
| Cross-node take offset | **Measured and written down.** No bar — this is the number we are honest about |
| Reference-loss detection | < 3 s to reported |

There is deliberately no pass/fail on cross-node take alignment. If it turns out
to be ±1 frame in practice, that is useful and still not a promise. If it is ±6
frames, that is more useful, because it tells an operator why two nodes should
not be asked to do one look.

## What this must produce

A sentence, verified, for `docs/INTEGRATION-NOTES.md` and for Configure's help
text:

> On our hardware, two nodes on a common reference held phase for an hour with no
> slip. Simultaneous takes across those nodes landed within ±N frames.

Plus confirmation of the mechanism claim in ADR 0011: **reference gives phase,
not frame numbering.** Two locked nodes will have wildly different frame counts,
and the capture should show exactly that.

## If phase does not hold

- Check the reference format matches what each card expects — a 1080i5994
  reference driving a 1080p5994 output is a common and confusing misconfiguration.
- Check the DA and the cable runs before suspecting software.
- If a card locks but drifts, that is an `INTEGRATION-NOTES.md` entry about that
  card, and possibly a reason not to use it for keyed output.

## Result

*Not yet run.*

```
Date:
Machines (model, OS, DeckLink model, Desktop Video version):
Reference generator and distribution:
Reference format:
Phase drift over 1 h:
Fill+key phase on one device:
Cross-node take offset (mean / p95 / max), server-fired:
Cross-node take offset, GPI-fired:
Reference-loss detection time:
Notes:
```
