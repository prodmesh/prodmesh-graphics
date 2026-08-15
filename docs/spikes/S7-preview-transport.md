# S7 — Preview encode and transport

**Gates:** M3 · **Risk:** [R7](../RISKS.md#r7) · **ADR:** 0001
**Status:** not started

## Question

What is the real glass-to-glass latency of node-rendered preview reaching the
browser, and what does it cost the node with several operators watching?

## Method

**Under test: WebCodecs + H.264 over WebSocket.** One connection, hardware encode
(VideoToolbox on macOS, NVENC where present), all-intra or short GOP so a joining
browser paints immediately, `VideoDecoder` to canvas.

MJPEG multipart is the M2 stopgap and is measured alongside as the baseline.
WebRTC is not tested — signalling, ICE, and an SFU question at eight viewers is
not worth it for a LAN, and it is deferred to P2, probably never.

1. Node downscales **the same GPU texture program uses** to 640×360, reads back
   **asynchronously** (never a blocking map), encodes, and ships over a socket on
   port 8521.
2. Browser decodes with WebCodecs and paints to canvas.
3. **Glass-to-glass:** put a running millisecond clock on the program output,
   point a phone camera at both the program monitor and the browser, and measure
   the difference from the photo. Crude and honest.
4. Repeat with 1, 3, and 6 browsers attached; measure node CPU, GPU, and encoder
   utilisation at each.
5. Measure how long a joining browser takes to paint its first frame.
6. **Starve the preview path deliberately** and confirm the program output is
   completely unaffected.
7. Repeat 3–5 with MJPEG for the baseline.

## Success bar

| Metric | Bar |
|---|---|
| Glass-to-glass, WebCodecs | ≤ 150 ms |
| Joining browser to first frame | ≤ 500 ms |
| Node cost at 6 viewers | ≤ 5% CPU, no effect on render frame time |
| **Program output when preview is starved** | **Zero effect. Preview drops; show continues** |

The last row is the only absolute. Everything else is a target.

## The non-negotiables this spike verifies

- Preview downscales **the same texture** program uses — not a second render.
  Anything else can drift from what goes to air.
- **Asynchronous readback.** A blocking buffer map on the render thread is the
  classic way to turn a preview feature into dropped output frames.
- Preview runs on a lower-priority queue with a hard budget. **If it exceeds the
  budget, drop the preview, log it, keep the show on air.**
- **Preview never shares the control socket.** A preview stall must not delay a
  cue ([ADR 0007](../decisions/0007-pmg-link-discovery-assignment-and-cues.md)).

## Why not MJPEG permanently

It works, it is trivial, and it is the right M2 choice. But it burns a browser
connection per stream, and a LAN appliance with no TLS gets no HTTP/2, so the
**six-connections-per-origin limit is real and unfixable at that layer** — which
is the exact lesson ProdMesh-Control learned in its ADR 0010. A Show view with
four preview panes plus the topic stream is already at the ceiling.

## If it fails

- **Latency too high** → shorten the GOP to all-intra, lower the resolution,
  check for encoder queueing.
- **Encoder unavailable** → software encode at 640×360 30 fps, or fall back to
  MJPEG with a documented viewer limit.
- **Node cost too high at 6 viewers** → encode once and fan out, which is the
  correct design anyway; verify it is not accidentally encoding per viewer.

## Result

*Not yet run.*

```
Date:
Hardware / OS / encoder:
Browser and version:
Glass-to-glass (WebCodecs):
Glass-to-glass (MJPEG):
Join-to-first-frame:
Node CPU/GPU @ 1 / 3 / 6 viewers:
Effect on render frame time:
Program output under preview starvation:
Notes:
```
