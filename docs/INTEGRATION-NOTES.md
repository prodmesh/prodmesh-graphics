# Integration notes

Device, SDK, and plant behaviour that cost real time to discover. Every entry is
**dated and hardware-attributed**, because the answer to "does DeckLink do X" is
almost always "on which model, which driver, which OS."

This file is written *after* something surprises us, and it is the first place to
look before re-deriving anything from a vendor PDF.

**Format:**

```
### <Short claim, stated as a fact>
*<date> · <hardware / OS / driver / version>*

What we observed, why it matters, and what the code does about it.
```

---

## Entries

### An older DeckLink driver silently reports a card as having no capabilities
*2026-08-14 · MacBook Pro arm64, macOS 26.3 · Desktop Video **14.5** · DeckLink SDK **16.0** · DeckLink Quad 2*

**The most expensive kind of bug: it looks like an answer, not an error.**

Blackmagic mints a **new IID whenever an interface changes**. An SDK-16.0
`IID_IDeckLinkProfileAttributes` (`F47551D7-…`) simply does not exist in a
Desktop Video 14.5 driver, so `QueryInterface` returns `E_NOINTERFACE`
(`0x80000004`).

What that looks like downstream is the problem. Every attribute read then falls
back to its default, and the card reports:

```
sub-device -1 of 0 · external keying no · max audio channels 0 · persistent id 0x0
```

A DeckLink Quad 2 — a card that unambiguously does external keying — presented
as a device with no capabilities whatsoever. Nothing errored.

**Fix, in `sys/decklink-sys/shim/pmg_decklink.cpp`:** query IIDs newest-first
and fall back. With the chain in place the same card reports
`sub-device 0 of 8 · 16 audio channels · reference input yes · persistent id 0xC55080A0`.

This driver satisfied: `attrs=15.3.1 status=15.3.1 output=15.3.1`.
`pmgctl decklink list` prints that line, and warns loudly if any reads `none`.

**The constraint that makes the fallback safe** — and the trap for whoever
extends it: casting a `_v15_3_1` pointer to the current interface type is only
valid for methods at *identical vtable slots*. Verified for what we call:

| Interface | Methods used | Slots identical across |
|---|---|---|
| ProfileAttributes | `GetFlag`=0, `GetInt`=1 | current, v15_3_1 |
| Status | `GetFlag`=0, `GetInt`=1 | current, v15_3_1 |
| Output | `GetDisplayModeIterator`=2 | current, v15_3_1, v14_2_1, v11_4, v10_11 |

**`IDeckLinkOutput` diverges at slot 6** — v14_2_1 inserts
`SetVideoOutputFrameMemoryAllocator`. Calling past the common prefix is a silent
vtable mismatch, not a compile error. Spike S3's output path should **narrow to
one version and require a matching driver** rather than extend this chain;
enumeration is the different case, because it has to work on whatever driver is
in the building.

This is the same lesson as ProPresenter 21.1 in the church versus 21.4 on the
laptop: **the building is not the dev machine.**

### Updating Desktop Video makes every DeckLink vanish until you approve the extension and reboot
*2026-08-14 · MacBook Pro arm64, macOS 26.3 · Desktop Video 14.5 → 16.3 · DeckLink Quad 2 in an Echo Express SE-I TB3*

A second instance of the same hazard as the entry above — userspace and driver
at different versions — and this time it presents as **the hardware being gone**.

After the update, `pmgctl decklink list` reported `driver loaded true` and
`devices 0`. The Thunderbolt chassis was still connected and healthy. What had
actually happened:

```
DeckLinkAPI.framework                      16.3   ← updated
com.blackmagic-design.BlackmagicIO.DExt    14.5   [activated enabled]     ← still live
com.blackmagic-design...Network.DExt       16.3   [activated waiting for user]
```

macOS holds a new DriverKit extension at **`activated waiting for user`** until
someone approves it in System Settings → Privacy & Security. Until then the old
extension stays live — so a 16.3 framework talks to a 14.5 dext and **nothing
enumerates at all**.

Indistinguishable, from the application's side, from a card that has been
removed. Diagnose with:

```
systemextensionsctl list | grep -i blackmagic
```

**Fix: approve the extension, then reboot.** `pmgctl decklink list` now prints
this whole diagnosis when it finds zero devices, because the failure looks like
absent hardware and nobody would think to check.

Operationally this is worth knowing before a Sunday: **updating Desktop Video is
not a self-completing operation.** It leaves the machine in a state where video
I/O is silently dead until a human clicks Allow and restarts.

### …and the reboot may not fix it: the old driver extension can deadlock holding the card
*2026-08-14 · same machine, immediately after the above*

After approving the extension and rebooting, still zero devices. The state was:

```
com.blackmagic-design.BlackmagicIO.DExt (16.3)  [activated enabled]
com.blackmagic-design.BlackmagicIO.DExt (14.5)  [terminating for upgrade via delegate]
```

which reads as "16.3 is live". **It was not.** The only running dext process came
from the bundle staged in April 2025 — the 14.5 one:

```
ps -ax -o args= | grep BlackmagicIO.DExt
  /Library/SystemExtensions/E661202C-…/…BlackmagicIO.DExt   ← staged Apr 2025 = 14.5
  (the 16.3 bundle, staged today, was not running at all)
```

**A deadlock.** The old extension cannot finish terminating because it is holding
the PCIe device; the new one cannot claim the device because the old one has it.
Rebooting does not break the cycle — on boot the old dext matches the card again
before the upgrade completes.

Note the PCI device itself was healthy throughout:
`pcibdbd,a13f@0 … registered, matched, active` (vendor `0xbdbd` = Blackmagic,
two functions on a Quad 2). The hardware was never the problem.

**Fix, least invasive first:**

1. **Unplug the Thunderbolt chassis, wait ~10 s, plug it back in.** Removing the
   device is what lets the old dext release and terminate. No password needed.
2. Failing that, force it out and re-activate:
   ```
   sudo systemextensionsctl uninstall 9ZGFBWLSYP com.blackmagic-design.BlackmagicIO.DExt
   ```
   then reboot, open Desktop Video Setup to re-request activation, approve,
   reboot again.

**How to tell the truth from `systemextensionsctl`:** it reports what is
*registered*, not what is *running*. Compare against the actual process:

```
systemextensionsctl list | grep -i blackmagic     # what macOS thinks
ps -ax -o args= | grep BlackmagicIO.DExt          # what is actually driving the card
ls -ldT /Library/SystemExtensions/<uuid>          # stage date tells you which version
```

If those disagree, the upgrade is wedged. This is the third distinct
userspace/driver version-skew failure found in one afternoon, and the only one
where the tooling actively reports a healthy state that is not true.

**Confirmed fix (same day):** unplugging the Thunderbolt chassis and plugging it
back in broke the deadlock. The running dext became the 16.3 bundle, and the new
driver immediately began a **card firmware update** — which is itself the proof
the handover worked, since it could only read the card's firmware version once it
owned the device.

**Budget for a firmware update after any Desktop Video upgrade, and for a full
power cycle after that** — card firmware is re-read at power-on, so a warm
restart can leave it reporting the old version. Do not interrupt the flash.

Total elapsed for what looked like "the card died": approve extension → reboot →
still nothing → unplug/replug → firmware update → power cycle. **None of it is
automatic, and all of it is invisible from the application's side.** Schedule a
Desktop Video update for a weekday, never the morning of a service.

### On a Quad 2, external keying is reported on only 2 of 8 connectors
*2026-08-14 · same hardware as above*

With the interface fallback in place, capabilities as actually probed:

| Connector | Sub-device | External keying | Reference in | Audio ch |
|---|---|---|---|---|
| [0] [2] [4] [5] [6] [7] | 0,2,4,5,6,7 | **no** | yes | 16 |
| **[1] [3]** | 1, 3 | **yes** | yes | 16 |

So a fill+key pair on this card lives on connectors **[1]** and **[3]** — not
wherever is convenient. This is exactly why `pmg_decklink.h` rule 3 says
capability is **probed, never assumed**: the model number does not predict it,
and neither does connector adjacency.

### Neither persistent_id nor topological_id is "the card" — grouping is unresolved
*2026-08-14 · same hardware · **open question, do not build on this yet***

Pairing has to mean *two connectors on one card*, because one card is one clock
and that is what actually guarantees phase (ADR 0011). SDK 16.0 has no
"paired device" attribute, so grouping is ours to do — and both candidate IDs
encode the sub-device in their low bits:

```
persistent_id   0xC55080A0 .. 0xC55080A7     one per connector
topological_id  0x500500..03, 0x500700..03   two prefixes, four each
```

`pmgctl decklink list` currently groups on `topological_id >> 8` and **labels it
a measured heuristic**, because that is what it is. It is not documented
behaviour and it has not been confirmed against a scope.

**Before Configure ever offers a fill+key pair as phase-locked, spike S3 must
confirm on a scope that the two connectors genuinely share a clock.** A pair
that silently spans two cards would look completely fine and be out of phase.

### DeckLink Quad 2 display modes, as reported
*2026-08-14 · same hardware · connector [1]*

16 modes. Rational rates throughout, which is why `pmg_dl_mode` carries
`fps_num`/`fps_den` and never a float:

```
 720 x 486    30000/1001i
 720 x 576    25000/1000i
1280 x 720    50000/1000  60000/1001  60000/1000
1920 x 1080   24000/1001  24000/1000  25000/1000  30000/1001  30000/1000
              50000/1000  60000/1001  60000/1000  25000/1000i 30000/1001i 30000/1000i
```

1080p59.94 is `60000/1001` = 59.9401 fps exactly. No UHD modes on this card.

Reference input is **present but not locked** — nothing is feeding it today, so
outputs would free-run. Spike S4 needs a generator before it can say anything.

### DeckLink scheduled output from Rust holds 1080p59.94 for ten minutes with zero late frames
*2026-08-14 · MacBook Pro arm64, macOS 26.3 · **Desktop Video 16.3** · DeckLink SDK 16.0 · DeckLink Quad 2, connector [1], Echo Express SE-I TB3*

Spike S3's success bar, met on the internal clock:

```
scheduled          35964      (10 min at 1920x1080p60000/1001)
completed          35964
DISPLAYED LATE     0
dropped            0
effective rate     59.951 fps (asked 59.940)
```

**This is the result that validates ADR 0003.** A hand-written C shim over the
DeckLink COM-alike, driven from safe Rust, sustains broadcast output without
dropping a frame. The fallback path was not involved — with 16.3 installed the
interfaces matched the SDK exactly (`attrs=16.0 status=16.0 output=16.0`).

Measured facts worth keeping:

- **Preroll of 3 frames is sufficient** at 1080p59.94 on this hardware. The
  header suggested 3–5; 3 held for ten minutes.
- **Buffered depth sat steady at 7–8** against a pool of 8 for the whole run.
  The scheduler was always ahead of the hardware, so **the frame pool was the
  limiting resource, not DeckLink**. If a future render path is slower than a
  memcpy, that headroom is what it eats into.
- **Rate accuracy is excellent**: 59.951 vs 59.940 over ten minutes, ~0.02%.
  With `clock_video` the SDK's pacing is trustworthy and we do not need a timer
  of our own.
- The scheduling loop backed off 89,754 times over 35,964 frames — roughly 2.5
  waits per frame — which is the correct shape: it means we were always waiting
  on the hardware rather than the hardware waiting on us.

**What this does NOT establish**, and both need a reference generator (the Evertz
at church — there is none at the house):

1. The same soak **locked to blackburst or tri-level**. The card reports a
   reference input present but unlocked, so this run was free-running.
2. **Scope confirmation that a fill+key pair genuinely shares a clock.** The
   connector-to-card grouping is still a measured heuristic (see the entry
   below), and Configure must not describe a pair as phase-locked until someone
   has seen it on a scope.

### Native wgpu and browser WebGPU are BIT-IDENTICAL on Metal — and that claim is narrower than it sounds
*2026-08-14 · MacBook Pro M3 Max, macOS 26.3 · wgpu 30.0.0 · Chrome 148 (Electron 42) · adapter reports `vendor: apple, architecture: metal-3`*

Spike S1, all three corpus figures:

```
                    within 1/255   max delta   centroid shift   alpha mass
triangle              100.0000%      0/255      0.000000 px      0.0000%
rounded-rect          100.0000%      0/255      0.000000 px      0.0000%
alpha-atlas           100.0000%      0/255      0.000000 px      0.0000%
```

Not "within tolerance" — **identical**. Every byte.

**What this actually establishes.** Chrome's WebGPU on this machine reports
`vendor: apple, architecture: metal-3`: Dawn is going to **the same Metal driver
on the same GPU** that wgpu-native uses. So this is *not* evidence that wgpu and
Dawn agree across backends. It is evidence that **two independent WGSL compiler
frontends — naga and Dawn's — emit shaders that produce identical results on one
driver**, and that nothing in our pipeline accidentally depends on backend-defined
behaviour.

That second half is the part worth having. It confirms the determinism rules in
`parity.wgsl` are doing their job:

- No `fwidth`/`dpdx`/`dpdy` — derivative granularity is implementation-defined,
  so AA built on it would differ *by specification*. Pixel scale is a uniform.
- No MSAA — sample patterns are unspecified across backends.
- `Rgba8Unorm`, never `Rgba8UnormSrgb` — no automatic colour conversion in the
  pipeline.
- Every sampler field stated explicitly; no filtering defaults.

**What it does NOT establish**, and must not be quoted as if it did:

- Cross-vendor behaviour (NVIDIA, AMD, Intel).
- Cross-backend behaviour (D3D12, Vulkan). **A Windows designer against a Mac
  node is the untested case**, and it is a real deployment shape.
- Anything about text. Glyphs are rasterized on the CPU in `pmg-text`, so they
  are identical by construction; the atlas figure covers the GPU's contribution
  (sampling). Typography *quality* is spike S2's problem, not this one.

For this project the tested case is arguably the important one — booth machines
are Macs — but ADR 0001's promise should still be quoted as **measured on Metal**
until someone runs the corpus on Windows.

**The harness was validated by making it fail.** A perfect score on first run is
a reason for suspicion, not celebration, so the corpus was perturbed deliberately:

| Perturbation | Channel delta | Centroid shift | Diagnosed as |
|---|---|---|---|
| Triangle, +3/255 on red | **3/255 FAIL** | 0.000000 | colour differs, geometry did not |
| Rect, shifted 1 px left | 252/255 | **1.000000 px FAIL** | geometry moved |

That separation is the whole reason the two metrics are reported independently.
A combined score would have called both "a failure" and hidden which one matters.

Browser-side timings, which also serve spike S6: **221.6 ms** for the first
figure (including adapter and device acquisition), then **5.1 ms** and **5.7 ms**.
The wasm bundle is **244 KB** — before any text stack.

### The GPU scene path renders a lower third in 13.9 ms against a 16.68 ms budget
*2026-08-14 · MacBook Pro M3 Max, macOS 26.3 · wgpu 30 · 1920×1080p59.94*

`pmgctl take` on the reference lower third — four nodes, 31 cached glyphs, a
1024×64 coverage atlas:

```
armed in         0.3–2.1 ms   (layout, binding, shaping, rasterization, atlas)
worst frame     13.89 ms      (budget 16.68 ms)
frames             103        In 800 ms → hold → Out 420 ms
```

Read that against the **17.55 ms** a CPU-generated 1080p frame took over the NDI
path (entry below). The difference is entirely the arm/take split: layout,
shaping, and rasterization happen **once**, and each frame only evaluates
timelines and applies transforms.

The worst-frame figure still includes a **blocking readback** — map, wait, copy —
which is the obvious thing to make asynchronous when a real output loop needs the
headroom. It is comfortably inside budget as it stands.

### The NDI runtime exports flat C symbols, not just the v5_load table
*2026-08-14 · MacBook Pro arm64, macOS 26.3 · NDI **6.3.2.0** (SDK APPLE, Apr 13 2026)*

`nm -gU libndi.dylib` shows **114 flat `NDIlib_*` exports** —
`NDIlib_send_create`, `NDIlib_send_send_video_v2`, `NDIlib_recv_capture_v3`,
and so on — alongside `NDIlib_v5_load`.

That matters for the binding strategy: resolving each symbol by name means a
mistake is a **named missing symbol**, not a silently wrong offset into a
~100-entry function-pointer table. `sys/ndi-sys` dlsyms each one eagerly at load
for that reason.

Runtime copies found on this machine, in the order `ndi-sys` tries them:
`/Library/NDI SDK for Apple/lib/macOS/`, then
`/Applications/NDI Launcher.app/Contents/Frameworks/`. Resolume, DashBoard, and
ProPresenter each ship one too, so a machine with any of them can run a node
without a separate SDK install.

### NDI struct layouts, verified rather than eyeballed
*2026-08-14 · same hardware · NDI SDK for Apple*

`NDIlib_video_frame_v2_t` is passed by pointer, so a wrong field offset is a
crash or garbage — not a compile error. A C probe compiled against the real SDK
headers emitted the authoritative offsets:

```
NDIlib_video_frame_v2_t  size 72  align 8
  xres 0  yres 4  FourCC 8  frame_rate_N 12  frame_rate_D 16
  picture_aspect_ratio 20  frame_format_type 24  [pad]  timecode 32
  p_data 40  line_stride_in_bytes 48  [pad]  p_metadata 56  timestamp 64
NDIlib_send_create_t 24 · NDIlib_source_t 16 · NDIlib_find_create_t 24
NDIlib_recv_create_v3_t 40
FourCC BGRA = 0x41524742 · progressive = 1 · BGRX_BGRA = 0 · bw highest = 100
```

These are asserted in `ndi-sys`'s `abi` tests against **recorded constants**, so
the tests run on CI machines with no NDI SDK installed. Then confirmed at
runtime: `pmgctl ndi probe` against NDI Test Patterns decoded 1920×1080, BGRX,
60/1, stride 7680 — and against our own sender, 1920×1080, BGRA `0x41524742`,
60000/1001, stride 7680.

**Static offsets plus a live decode is the pattern to reuse** for any
pass-by-pointer vendor struct.

### A CPU-generated 1080p frame does not sustain 59.94 — 17.55 ms against a 16.68 ms budget
*2026-08-14 · MacBook Pro arm64, macOS 26.3 · NDI 6.3.2.0 · release build*

`pmgctl ndi send --pattern softedge` at 1920×1080, `clock_video: true`:

```
Sent 35964 frames in 631.35s = 56.964 fps (asked for 59.940)
```

**17.55 ms per frame against a 16.68 ms budget** — about 5% short, sustained
over ten minutes. This is *not* an NDI problem: the SDK paces correctly and we
simply miss the deadline.

The cause is that the sender regenerates the pattern **every frame on the CPU**,
including a per-pixel `sqrt` for the soft edge, plus an unpremultiply and a
linear→sRGB encode over 2.07M pixels. That was deliberate — the spike keeps the
work honest rather than caching a static frame — and the number is the point.

Two things it tells us, both already assumed but now measured:

1. **A static graphic must never be re-rasterized per frame.** Arm builds the
   `SceneInstance` and rasterizes; take and the timeline animate transforms over
   the cached result. This is exactly the `layout.*` vs `transform.*` split in
   `docs/specs/scene-format.md` §3.2, and here is what ignoring it costs.
2. **The per-frame encode belongs on the GPU.** Unpremultiply and transfer
   encode are per-pixel and trivially parallel; on the CPU they eat a frame
   budget on their own. ADR 0002's renderer does this in a shader.

Useful as a floor for spike S3: if a CPU memcpy-and-encode path is already at
105% of budget at 1080p59.94, DeckLink's scheduled output has no headroom for
one.

---

## Inherited notes worth knowing

Not ours, but relevant, and cheaper to read here than to rediscover.

### ProPresenter's HTTP API port is per-machine and ephemeral
*From ProdMesh-Control's `docs/INTEGRATION-NOTES.md`*

Seen on 1025 and 62201; 62202 is that module's fallback default; 49310 is the
legacy WebSocket and does not speak HTTP. Relevant here if ProPresenter ever
becomes a data source for graphics (a P2 input type).

### ProPresenter coalesces rapid slide advances
*From ProdMesh-Control*

A fast pass through cues arrives as a jump, not as each intermediate state. Any
future auto-trigger from ProPresenter state must be edge-tolerant, not
edge-dependent.

### Our campus runs ProPresenter 21.1; the dev laptop runs 21.4
*From ProdMesh-Control*

`/v1/playlist/active` can answer all-null on 21.1, and uuid-based playlist
resolution no longer works there — index paths only. Same lesson applies to
anything we integrate: the building is not the laptop.

---

## Sub-pixel motion stutters ~0.2 px, and it is not a text problem

**Measured 2026-08-15, MacBook Pro M3 Max / macOS 26.3 / wgpu 30, `pmgctl spike s2`.**

Sliding an element by a known sub-pixel amount does not move its ink by that
amount. The centroid follows a staircase with a 1.0 px period: it lags for half
a pixel, jumps ~0.44 px as the edge crosses a pixel centre, then tracks. Worst
deviation from linear is **0.33 px for text** and **0.21 px for a plain
antialiased box**. Whole-pixel motion is exact (255.500 → 257.500 over 2.0 px).

**Do not assume this is glyph-atlas sampling.** The box control uses an analytic
SDF and samples no texture at all, and it stutters too. The fault is in the
geometry/coverage path and affects **every** animated element — which matters
because ADR 0005 makes "animate transforms, not layout" the fast path for
everything.

**Two hypotheses already spent, so nobody spends them again:**

- *`smoothstep` coverage.* Point-sampling a cubic where true coverage is linear
  in signed distance. Switched `scene.wgsl` to `clamp(0.5 - d, 0, 1)`:
  **0.214 → 0.205 px.** Not the cause. The linear ramp is kept anyway because it
  is the correct coverage function.
- *Atlas bilinear filtering.* Ruled out by the box control above.

Still open, in the order worth trying: 8-bit alpha quantisation of a one-pixel AA
band; an AA band too narrow to carry sub-pixel information across a pixel centre;
and the centroid metric itself saturating when an edge column clamps at 0 or 255.

**Whether 0.2 px is actually visible is not yet known** — that needs eyes on a
real output at a real reveal speed. What exists now is a number, so any fix can
be shown to have helped. See `docs/spikes/S2-text-typography.md`.

## Text metrics that pass every relative assertion can still be wrong

**2026-08-15.** swash's `linear_scale()` returns **raw design units**; `scale(ppem)`
divides by `units_per_em` first. Using the former gave an ascent of 85,284 "px"
at a 46 px size.

Every test passed. They compared metrics **to each other** — ascent > 0,
descent < 0, line height ≥ ascent − descent — and all of those relationships hold
perfectly when every number is wrong by the same factor of 2048.

**A suite that only compares values to each other cannot catch a units error.**
`pmg-text` now asserts absolute plausibility bounds (an ascent at 46 px is
between 20 and 60 px) alongside the relationships.

---

## CBOR across Rust and JavaScript breaks at 2³², in both directions

**Measured 2026-08-15, ciborium 0.2 + cbor-x, building the node link client.**

JavaScript has one number type and CBOR has several. cbor-x picks one by
magnitude, and it picks differently on each side of the boundary. Both failures
below shipped through a full test suite before a real node hit them.

**Decode (Rust → JS).** ciborium writes a Rust `u64` as a 64-bit CBOR integer.
cbor-x decodes that to a **BigInt** by default. A BigInt is not `===` any
Number, throws when mixed into arithmetic, and dies in `JSON.stringify` — so
`at_utc` looked fine in every unit test and would have failed on the first real
`CueAck`. Fixed with `int64AsNumber: true`.

**Encode (JS → Rust), which is worse.** cbor-x writes an integral JS number
**above 2³²** as a **float64**. `Date.now()` is above 2³², so `Ping { t_ms }`
and `Welcome.time_ref` went out as CBOR floats, and serde refuses a float where
a `u64` is declared:

```
WARN undecodable frame error=decode: Semantic(None,
     "invalid type: floating point `1786780546347.0`, expected integer")
```

The node logged that once a second, **never decoded a single `Welcome`**, and
therefore never learned the server's epoch — while otherwise appearing to work:
it linked, staged assets, took graphics on air. Fixed by converting integral
numbers to BigInt on the way out (`server/wire.js::intify`), which makes cbor-x
emit a CBOR integer.

**And the correction that fix required.** With integers going out as integers, a
JSON `3` destined for `DataValue::Number(f64)` arrived as a CBOR integer and
serde refused it the other way — *"invalid type: integer `3`, expected float"*.
So: **every float on this wire is lenient and accepts an integer** (`lenient::f64`
in `pmg-proto`), because JavaScript cannot express the distinction. Every
**integer** stays strict: a `u64` arriving as a float means something upstream
lost precision and should be loud.

**The process lesson, which is the durable one.** The Rust→JS fixtures could not
catch the encode bug, because they only tested one direction. **A wire contract
needs a checked-in fixture each way** — `testdata/proto/*.cbor` from Rust and
`js-*.cbor` from `npm run wire:fixtures`. The JS→Rust test found the third bug
above within seconds of being written.

## An unattended process that cannot log cannot be diagnosed

**2026-08-15.** `pmg-node`'s `tracing_subscriber_init` was a stub that flushed
stderr and installed nothing, so every `tracing::warn!` in the crate went
nowhere. When the node appeared to die on server loss there was **no output at
all** to reason from.

The apparent death turned out to be the test harness — `lsof -ti:8599` matches
processes with a socket *to or from* the port, which includes the node's own
client connection, so `kill -9` on that list killed the node. `-sTCP:LISTEN`
selects only the listener. But that was invisible until logging worked.

For a process whose whole job is to run unattended in a booth, a real subscriber
is not a nicety.
