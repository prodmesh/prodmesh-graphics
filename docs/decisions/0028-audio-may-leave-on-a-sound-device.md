# ADR 0028 — Audio may leave on a sound device, and then there are two clocks

Status: accepted · 2026-08-22

## Context

Audio only ever left this system **embedded in the video** — in SDI or in NDI.
That is right for a plant whose audio arrives on the same cable as the picture,
and it is no use at all to one whose audio arrives over Dante: a quarter of the
reference rundown's audio goes to a Dante Virtual Soundcard, which is a
CoreAudio device, and there was no way to reach it.

`docs/ROADMAP.md` has carried the gap since M3 — *"an audio-only output backend
… today audio only leaves this system embedded in SDI or NDI"* — and it was
asked for directly: **video over SDI, audio out to DVS.**

## Decision

**An output's audio has a destination: embedded, or a named host sound device.**

`audio_device` and `audio_host` on the output configuration, empty meaning
embedded — which is what every existing output means, so nothing changes for
anybody who does not set them. It is HARDWARE and lives in the node's own file
beside the DeckLink device (ADR 0024); changing it restarts that output's render
group, because closing one device and opening another is what it is.

### One destination, never both

The temptation is to embed *and* send to a device, and it is wrong for a
concrete reason. The per-frame sample count is corrected against exactly one
buffer. A second destination would have nothing holding it in step, so it would
be open loop — and an open loop is the failure this whole module is shaped
around: **measured at +168 ms over a two-hour soak**, with nothing correcting it.

Two destinations is therefore not a feature to add later. It is two clocks with
one correction, and it would drift.

### The video output is still the clock. The device reports the error

`pmg-audio` rests on one rule: audio has no rate of its own, it has a number of
samples that must accompany each video frame. That does not change here. What
changes is where the *error* is read from:

- **Embedded:** the DeckLink's own queue depth.
- **On a device:** the sink's ring level.

Either way `AudioSync` picks the count actually sent, correcting by one sample a
frame — inaudible — and hard-resyncing past 100 ms. The loop is the same loop;
only the sensor moved.

**And the two clocks are genuinely independent.** DVS is clocked by the Dante
network's PTP master, a DeckLink by its reference input. Unless the house locks
both to one master they *will* walk apart. That is not a defect to be fixed in
software; it is two crystals, and the only question is whether the drift is
corrected continuously or discovered at the end of a service.

### The ring is lock-free, and the device is primed before it drains

The consumer is a host audio callback at real-time priority with a hard
deadline. **It may not allocate and it may not wait.** A mutex there is not a
performance question: if the render thread is preempted holding it, the callback
misses its deadline and the plant hears a click. So: one allocation at open, two
atomics, no blocking path.

Priming is the other half, and it was found by measuring rather than by
thinking. A DeckLink prerolls — schedule N frames, then start. cpal has no
preroll, so the first implementation started draining from the first callback,
at a depth of nothing: **268 underruns in two minutes**, with the buffer never
climbing above a couple of milliseconds. The callback now outputs silence until
the ring first reaches its working depth. Those callbacks are not underruns;
nothing was late, the buffer was filling.

### A device that will not open does not take the picture off air

Loud, and not fatal. The output runs silent and the node says which device and
why. A show continuing without sound is recoverable in a way that a show with no
picture is not.

### Which host APIs, and what about MME and DirectSound

cpal: **CoreAudio** on macOS, **WASAPI** on Windows, **ALSA** on Linux, with
**ASIO** behind a feature flag — off by default, because it needs the ASIO SDK
present at build time and a build that silently requires a vendor download is
the `sys/decklink-sys` problem again (ADR 0003).

**MME and DirectSound are deliberately absent.** They are legacy paths to the
same Windows device that WASAPI reaches, and Dante Virtual Soundcard presents on
all of them; WASAPI is the one with the lowest latency and the clearest
buffer-level reporting, which is what the correction loop reads. The host is a
named, enumerated value in this API — not an assumption — precisely so adding
one later is additive and a stored configuration keeps meaning what it said.

The host is part of a device's IDENTITY, not decoration: "Speakers" on WASAPI
and "Speakers" on ASIO are different devices with different clocks.

## The alternatives, and why they lose

**Resample to the device's clock instead of correcting the sample count.** The
textbook answer, and it costs a resampler in the path for every output plus a
quality question nobody wants to have about a service. Correcting by one sample
a frame is below audibility and needs no filter at all.

**Let the sound device be the clock and drive video from it.** Coherent, and it
inverts the rule the entire renderer is built on — the video output paces the
loop because a late frame is the thing that must never happen. Audio is the
thing that can absorb a sample.

**A separate audio-only output with no video.** The roadmap's phrasing, and it
is a routing feature rather than a transport one: it needs an answer to "which
layers' audio" that this decision does not have to give. The transport built
here is what such an output would use, so it is not foreclosed.

**Ship without the drift loop and see.** The measurement already exists: +168 ms
over two hours, open loop, on the *embedded* path where both clocks are on one
card. Two independent crystals will not be better.

## Consequences

- `pmg-out-audio` is a new crate with one non-workspace dependency, `cpal`. It
  is a transport and takes no dependency on `pmg-audio`; the 48 kHz rate is
  stated in both and pinned by a test.
- The node probes sound devices at boot and reports them, so Configure's picker
  is driven by what the machine has — the lesson from the DeckLink probe, which
  stood empty for four milestones behind a comment.
- `OutputState` gains `audio_out`: device, buffer level, underruns, drops,
  corrections and resyncs. **Reported, never hidden** — a node quietly
  correcting all service is a node with something else wrong, and an underrun is
  an audible click that appears nowhere else in the system.
- **The cushion is three video frames — about 50 ms — and a machine under full
  load eats it.** Measured: 0 underruns idle over three minutes and over a
  ten-minute soak; 31 in 45 seconds with all twelve cores pegged; recovery
  without a resync, and **zero late video frames throughout**. The audio is the
  thing that gives first, which is the right way round. The depth is not
  exposed: deepening it costs latency for everybody to survive a case that
  should not happen on a booth machine. See `docs/INTEGRATION-NOTES.md`.
- `pmgctl audio list` and `pmgctl audio soak` are the instruments, in the same
  shape as `pmgctl decklink`. The soak is the only place the open-loop cost can
  actually be seen: `--free-run` turns the correction off on purpose.

## Seam

- **A plant-delay offset per output.** `docs/specs/output-config.md` §7 already
  reserves one and this does not implement it: a fixed offset is a ring
  pre-charge, which is a few lines, and there is no measurement yet to say what
  anybody needs.
- **An audio-only output** — no video, audio only. It would need this transport
  and a rule for which layers feed it.
- **Channel routing** — which internal playback channels reach which device
  channels. ADR 0024's seam already names this; it belongs beside
  `audio_channels` and is still not designed.
- **ASIO**, by turning the feature on where the SDK is present. Lower latency,
  not access: DVS is reachable over WASAPI without it.
- **The buffer depth**, if a node ever has to share a machine with something
  heavy. One number, per output, beside the audio device.
- **A second destination**, if anybody ever genuinely needs embed-and-send. It
  needs an answer to which buffer the correction follows, and "both" is not one.
