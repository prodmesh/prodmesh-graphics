# S6 — WebGPU in the booth's browsers

**Gates:** M0 → M1 · **Risk:** [R6](../RISKS.md#r6) · **ADR:** 0001
**Status:** not started

## Question

Is WebGPU usable for the Design canvas, **on the actual machines in our booths** —
not on a development laptop? What are the real init time, bundle size, and frame
time, single-threaded?

The single-threaded constraint is not a simplification, it is the situation:
`SharedArrayBuffer` and WASM threads require COOP/COEP headers, which conflict
with a plain-HTTP LAN appliance that deliberately has no TLS.

## Method

1. Build `pmg-wasm` with a hello-triangle plus one shaped text block, the same
   corpus S1 uses.
2. Serve it from the control server over plain HTTP — **no COOP/COEP, no TLS,
   single-threaded**, exactly as it will ship.
3. Load it on **the actual booth Macs**, plus a dev laptop for comparison, in
   Chrome, Safari, and Firefox.
4. Measure:
   - Time from navigation to first rendered frame.
   - Gzipped and uncompressed bundle size.
   - Frame time rendering a 1920×1080 offscreen lower third.
   - Frame time at half resolution — the resolution the designer preview will
     actually use.
   - Peak memory.
5. Enumerate the reported WebGPU limits on each machine and compare them to what
   the shaders assume.
6. Load fonts **as bytes from the CAS**, never via CSS, and confirm the text
   matches the native render.

## Success bar

| Metric | Bar |
|---|---|
| Chrome/Edge on booth hardware | Works; **≤ 33 ms/frame at half resolution** |
| Init to first frame | ≤ 3 s on booth hardware |
| Bundle, gzipped | ≤ 8 MB (LAN-served, so this is comfort not correctness) |
| Reported limits | Meet or exceed what the shaders need |
| Safari | Works, or a clear reason it does not |

Design mode requiring Chrome or Edge is an acceptable outcome and is already the
stated recommendation. Configure, Edit, and Show are ordinary DOM and must work
everywhere — that is not in question here.

## If it fails

- **Too slow at half resolution** → reduce preview to 30 fps and quarter
  resolution; the designer canvas does not need to be real-time video.
- **Limits below what shaders need** → write to the WebGPU downlevel tier and add
  a CI job validating against those limits, not against the dev machine's.
- **Unusable on booth hardware entirely** → ADR 0001's fallback: the design
  preview becomes a node-rendered stream. Costs latency and a node per designer;
  keeps exactness.

## Result

*Not yet run.*

```
Date:
Booth machine (model, OS, GPU):
Browsers and versions:
Init to first frame:
Bundle size (gz / raw):
Frame time @1080 / @half:
Peak memory:
Reported limits vs required:
Font-from-bytes matched native? :
Notes:
```
