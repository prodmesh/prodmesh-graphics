# Spikes

Each brief is **question / method / success bar / result**. The result section is
filled in when the spike runs — **dated and hardware-attributed**, per family
convention — and its findings are copied into `docs/INTEGRATION-NOTES.md` and
reflected back into `docs/RISKS.md`.

A spike that fails is a success. Three of these are scheduled in M0 precisely so
that ADRs 0001, 0002, and 0003 can be revisited before anything depends on them.

| Spike | Question | Gates |
|---|---|---|
| [S1](S1-render-parity.md) | Do native wgpu and browser WebGPU produce the same picture? | M0 → M1 |
| [S2](S2-text-typography.md) | Is Rust text good enough for broadcast, especially animated? | M5 |
| [S3](S3-decklink-output.md) | Can Rust drive DeckLink output cleanly for ten minutes? | M0 → M2 |

**S3's enumeration half ran on 2026-08-14** and produced three findings in
`docs/INTEGRATION-NOTES.md`, including one that would have made every DeckLink
look incapable. The output soak has not run.
| [S4](S4-genlock-phase.md) | What does reference actually phase-lock, across machines? | M3 |
| [S5](S5-media-decode-sync.md) | Can we seek and play frame-accurately with synced audio? | M3 |
| [S6](S6-webgpu-browser.md) | Is WebGPU usable in the booth's browsers? | M0 → M1 |
| [S7](S7-preview-transport.md) | What is glass-to-glass preview latency, and at what cost? | M3 |
| [S8](S8-cue-chaos.md) | Does the cue path survive chaos without losing or duplicating? | M2 |
| [S9](S9-color-measurement.md) | Is the colour and key output numerically correct? | M3 |
