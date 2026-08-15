# S5 — Frame-accurate decode and A/V sync

**Gates:** M3 · **Risk:** [R5](../RISKS.md#r5)
**Status:** not started

## Question

Can we seek to an exact frame and play video with frame-synced audio, in Rust,
without ever stalling the output?

## Method

**Stack under test:** ffmpeg via `rsmpeg`/`ffmpeg-next` for demux and decode,
with platform hardware acceleration where available. `symphonia` for audio where
it suffices. (Pure-Rust H.264/HEVC is not production-ready and is not a
candidate.)

**Test media**, each with a **burned-in frame number**:

| Clip | Why |
|---|---|
| ProRes 4444 with alpha, 1080p59.94 | The intended intermediate for cued media |
| DNxHR HQX with alpha | The other intended intermediate |
| HAP with alpha | The GPU-friendly option |
| H.264 long-GOP, B-frames, open GOP | The convenience case, and the hard one |
| A clip with 601 colour tags | Colour conversion on decode |
| A variable-frame-rate phone clip | Must be **rejected at ingest**, not accommodated |

Then:

1. **Seek accuracy** — seek to 100 random frames in each clip, decode one frame,
   and compare the burned-in number to the requested one.
2. **Sustained playback** — 30 minutes of 1080p59.94 ProRes 4444 into a DeckLink,
   with a lip-sync pattern, measuring A/V drift at start, 15 min, and 30 min.
3. **Underrun behaviour** — throttle the decode thread artificially and confirm
   the output holds the last frame, logs it, and **never stalls**.
4. **Alpha** — confirm the alpha channel survives decode and reaches the
   compositor premultiplied correctly.
5. Measure decode CPU and memory per stream, to establish how many video layers a
   node can carry.

## Success bar

| Metric | Bar |
|---|---|
| Seek accuracy, all-intra clips | **100/100 exact** |
| Seek accuracy, long-GOP H.264 | Documented, whatever it is — this is why we transcode |
| A/V drift over 30 min | < 1 frame |
| Decoder underrun | Output never stalls; last frame held; logged |
| VFR source | Rejected at ingest with a clear message |

**The clock is always the video output.** Audio resamples to fit. The decoder is
*pulled* by the output callback, never pushed. Any design where the decoder
drives timing fails this spike by construction.

## The decision this spike confirms

That **requiring an all-intra intermediate for cued media is right**, and that
the ingest transcode is worth building. Long-GOP H.264 is where frame-accurate
seek goes to die; the measurement in step 1 is what makes that a fact rather than
folklore, and it is what justifies the ingest pipeline in
`docs/specs/asset-cas.md` §3.

## If it fails

- **Inexact seek on all-intra** → a bug in PTS→frame mapping at non-integer frame
  rates. Ours to fix.
- **Drift** → the audio resampler is not being driven by the video clock. Ours to
  fix.
- **Underrun stalls the output** → the decoder is on the output's critical path.
  Architectural, and must be fixed before M3 ships.
- **Hardware decode unavailable or unreliable** → software decode with a
  documented layer-count limit.

## Licensing note

**LGPL ffmpeg components only, dynamically linked, no GPL.** Confirm this in the
spike's build, not later — linking `libx264` for convenience would quietly make
the whole binary GPL and foreclose `docs/VISION.md`'s posture permanently
([ADR 0003](../decisions/0003-rust-playout-node-with-c-shims.md) §Seam).

## Result

*Not yet run.*

```
Date:
Hardware / OS:
ffmpeg version and build config (confirm LGPL, no GPL):
Seek accuracy by codec:
A/V drift @ 0 / 15 / 30 min:
Underrun behaviour:
Alpha path verified:
Decode CPU/mem per stream:
Max concurrent video layers:
Notes:
```
