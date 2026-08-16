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

**Update, 2026-08-15 — the dev machine moved, and now the fallback is the
untested path.** Desktop Video on this MacBook Pro is now **16.3**, and the same
`pmgctl decklink list` reports `attrs=16.0 status=16.0 output=16.0`: the
newest-first query succeeds on its first try and the fallback chain never runs.
Capabilities read correctly and directly — `16 audio channels`, `persistent id
0xC55080A0`, external keying present.

That is worse for us, not better. **The code path that made a 14.5 driver work
is now exercised nowhere on this machine**, while the booth machine is the one
likely to still be on an old driver — which is exactly the configuration the
fallback exists for. The lesson has inverted rather than gone away: the dev
machine is no longer the building, so the compatibility path needs a deliberate
test rather than an incidental one. Until there is a way to run against an older
driver, treat the fallback as **unverified since 2026-08-14** rather than as
working code.

Also measured on the Quad 2 at 16.3, and relevant to fill+key configuration:
**external keying is present on only one connector per topological group** —
`[1]` in group `0x5007` and `[3]` in group `0x5005`, not on all eight. Configure
must not offer a keyed pair on a connector that cannot key.

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

---

## The designer and the CLI compile to the same bytes — measured, in a browser

**2026-08-15, Chrome / WebGPU on an M3 Max.** `compile_template` in the browser
produced

```
blake3:44dbd44c3bdc19e4d40d8affc35d225aa86fb6560a4639c1488756a4c183de6a  (2893 bytes)
```

for `testdata/templates/lower-third.json` — **identical** to what
`pmgctl scene compile` writes for the same document. That is ADR 0001's central
claim at the compiler level, and it is what lets Design publish a revision by id
and a node fetch it by the same id with nothing in between having to agree.

Also measured in-browser, same session: **arm 22.5 ms**, first frame 10 ms
(pipeline creation), **warm frames 3.34 ms average / 4.4 ms worst** across a
scrub. `visible_if` collapses correctly in the browser — clearing the title took
the band from 132 px to 94 px and reported `omitted: ["title"]`.

`crates/pmg-wasm/tests/same_compiler.rs` pins the compiler half in CI without a
browser, because `design.rs` builds natively too. It cannot check that the WASM
build agrees — that remains the parity harness's job.

## A React effect that runs twice can strand a fetch in the cache

**2026-08-15.** The sidebar sat on "connecting" while `/api/version` returned
200. `useQuery` returned early when it found a **fresh** cache entry, which is
correct for a settled one and wrong for an in-flight one.

React runs an effect twice on mount in development. The first run started the
fetch and registered a wake-up; its cleanup cancelled that wake-up; the second
run found the entry, saw it was fresh, and returned without attaching to the
pending promise. Nothing ever re-rendered, with the data sitting in the cache
beside the component.

It is not a development-only bug: the same thing happens in production whenever
a component remounts inside the 5 s stale window. Fixed by attaching to
`entry.promise` when the entry is fresh but unsettled, and pinned by a test that
renders under `StrictMode` against a fetch with a real delay.

---

## A layer stack needs three things the single-layer path did not

**2026-08-15, building M2a.** Adding a second layer surfaced three defects that
were invisible with one, and none of them reported an error.

**1. The renderer always cleared its target.** `SceneRenderer::render` began its
pass with `LoadOp::Clear`, so drawing layer 2 erased layer 1. The `StateReport`
said both layers were occupied — because they were. Only the pixels disagreed,
and the symptom would have been a Bug silently wiping the Lower Third under it.
Fixed with an explicit `Load::{Clear, Over}`: the bottom layer clears, every
layer above composites. `crates/pmg-render/tests/compositing.rs` asserts both
the fix and the old behaviour, so the bug is a pinned fact rather than a memory.

**2. One renderer, rebuilt per layer per frame.** The node held a single
`SceneRenderer` and rebuilt it whenever the layer being drawn changed — which is
every layer, every frame, the moment two are occupied. That is the 17.55 ms
regenerate-per-frame mistake in a different costume. Now a `RendererCache` keyed
by layer name, rebuilt only when the occupant changes and dropped when the layer
clears. Measured with two layers live: **59.94 fps, 1 late frame**.

**3. `configure` appended unknown layers instead of taking the assignment's
order — and a test had baked that in.** Layer order IS compositing order. A node
booted with `Lower Third` and then assigned `Fullscreen, Lower Third, Bug` ended
up compositing `Lower Third, Fullscreen, Bug`, so a fullscreen graphic would
have covered the lower third meant to sit on top of it. The existing test
asserted `layers[0] == "Lower Third"` after reconfiguration, which is exactly
the wrong expectation written down as a requirement. The assignment's order is
authoritative now; occupants are carried across by name rather than left in
place.

## A dead session must not shadow a live one with the same name

**2026-08-15.** Reinstalling a node gives it a fresh `node_id`, so the server
holds two sessions with the same display name — one dead, one live. `find()`
treated that as ambiguous and refused, answering *"There is no node called
booth-1"* while booth-1 sat there connected.

Name lookup now prefers the reachable session when exactly one match is online.
Two *live* nodes sharing a name is still ambiguous and still refused, because
picking one would be a coin flip deciding what goes on air.

---

## `invalidate()` that only deletes refreshes nothing

**2026-08-15, building M2b.** Three symptoms at once: two playlists both named
"Playlist 1", each holding one item when two were added, and a rundown that
stayed empty. One cause.

`useQuery.invalidate()` deleted the cache entry and stopped. A component that is
already mounted has an effect keyed on `[key]`; the key has not changed, so it
never looks again. **Every save-then-invalidate in the app was silently failing
to refresh**, and it looked like it worked wherever something happened to
unmount in between — closing an editor, switching modes. Where nothing
unmounted, the screen kept showing pre-save data and the next write was computed
from it.

Invalidation is an event now: subscribers are registered per key and told to
refetch. Pinned by a test that mutates the response between two renders of a
component that never unmounts.

The lesson generalises past this codebase: a cache with no subscriber registry
can be *populated* correctly and still never be *read* again, and the failure
looks like a UI bug rather than a data-layer one.

## A warning that fires every time is not a warning

**2026-08-15.** The rundown panel's divergence notice originally read "the
rundown is on X, but that isn't what's on air", triggered whenever the cursor
did not match the output.

`Take next` fires the cursor's item **and then advances**, so the cursor is one
ahead of the output almost permanently. The notice appeared on every single
take. Correct by its own definition, and useless — an operator learns to ignore
it within a service, at which point it may as well not exist.

The signal an operator actually needs is narrower: **the output is showing
something this rundown does not contain** — taken from the rack, fired by a GPI,
left over from earlier. Where they are *within* the running order is already
carried by the two marks in the list, which needs no prose. The notice now names
the stranger, so it says what to look at rather than that something is wrong.

---

## mDNS on macOS: EHOSTUNREACH means Local Network permission, and it is cached per process

**2026-08-15, corrected the same day.** The server advertises `_pmg-control._tcp`
and `pmg-node` browses for it when `--server` is omitted. Neither works on this
machine yet:

```
discovery: mDNS error (advertisement only, the server is fine) — send EHOSTUNREACH 224.0.0.251:5353
```

**The diagnosis, isolated to five lines.** A bare `dgram` socket sending one
datagram to `224.0.0.251:5353` fails `EHOSTUNREACH`, while a unicast send to
loopback from the same process succeeds. Nothing about the protocol, the
library, or the interface set is involved:

```js
s.send(Buffer.from('probe'), 5353, '224.0.0.251', cb)  // EHOSTUNREACH
s.send(Buffer.from('x'),     5353, '127.0.0.1',   cb)  // OK
```

That is **macOS Local Network Privacy** denying the process, which surfaces as
`EHOSTUNREACH` rather than `EPERM`. The app needs approval in
*System Settings → Privacy & Security → Local Network*, per responsible app —
which for a CLI is the terminal or whatever launched it — and the process must
be restarted after granting.

**What I got wrong first, recorded because the wrong version is instructive.**
The original note here claimed the host's many virtual interfaces (`anpi1`,
`anpi2`, `gif0`, VPN tunnels) meant the responder was sending on ones that could
not carry multicast, citing a `netstat` route table as evidence. That was a
plausible story assembled from circumstantial output, and it was wrong. A route
to 224.0.0.251 existing says nothing about whether *this process* is allowed to
use it. **`EHOSTUNREACH` on multicast with working unicast is an OS permission,
not a routing problem** — and the five-line test above distinguishes them in
seconds, which the route table never could.

**Resolved, and now verified end to end.** Granting Local Network to `node` in
System Settings and **restarting the processes** made both halves work: macOS's
own `dns-sd -B` sees our advertisement, and `pmg-node` with no `--server` finds
the server and links.

**The permission is cached for a process's lifetime.** After granting, a
still-running process keeps failing exactly as before — which is what made this
look unfixed for a while, and produced a second wrong claim ("zero errors now")
from grepping a log that was three seconds old. Grant, then restart, then test.

`--server` remains the supported path and always wins: *mDNS is an aid, never an
authority.*

## Discovery must not hand back a link-local address

**2026-08-15.** The first working discovery returned
`fe80::5cc2:f8ff:fef3:498e%en0:8519` — a link-local IPv6 address with a zone
index, chosen because the code took whichever address the responder listed
first.

That address does not just dial the control socket. It becomes the **base URL
for CAS fetches** and the host in the **preview URL handed to a browser**. A
percent sign and an interface name in a URL is a poor thing to hand to an HTTP
client, and on a multi-homed machine the zone is the wrong interface half the
time.

`best_address` now prefers **IPv4, then routable IPv6, and link-local only as a
last resort**, and brackets IPv6 literals because the CAS base URL is built by
formatting straight into one.

Related, found in the same run: the browse returned on the **first** responder,
so the "two control servers answered" warning could never fire. It collects for
a short grace period now and names every server it saw — a node cannot arbitrate
between two control servers, but the operator can, and only if they are told.

**The part that matters more than the feature.** `bonjour-service`'s default
error handler is `function (err) { throw err }`, called from inside a socket
callback — an uncaught exception on the event loop. On this machine that fires
repeatedly, so **an unreachable multicast group would have crashed the control
server**. A convenience feature must not be able to take down playout control;
`advertise()` now passes an error callback that logs and continues. Any future
optional network feature gets the same treatment before it ships.

---

## DeckLink `BUSY` on schedule is backpressure, not a fault

**Measured 2026-08-15, DeckLink Quad 2, Desktop Video 16.3, 1080p59.94.**

`pmg_dl_output_schedule` returns `PMG_DL_ERR_BUSY` (-5) when the card's
scheduling queue is full. That is **how a scheduled output paces its producer**
— it has not returned a frame yet — and it is the mechanism the node's frame
loop relies on instead of a timer of its own.

Treating it as an error makes the node exit on its very first frame with
`DeckLink call failed with code -5`, which reads exactly like a hardware fault
and is not one. `pmg-out-decklink` backs off a quarter of a frame and retries,
and only calls it a failure after ~2 s of a queue that never drains. The soak in
`pmgctl decklink out` had this right already; the new backend had to learn it
again.

## Two ordinary ports beat the card's paired mode

`KeyMode::FillKeyPair` is the card linking two connectors itself, and it needs a
`paired_port` the shim does not pass yet plus `supports_key_fill` on the device
— which on a Quad 2 is true for only **some** sub-devices (0 and 2 say no, 1 and
3 say yes).

`pmg-out-decklink` opens **two ordinary outputs and drives both**. It needs no
keying capability, works on any two SDI outs, and sidesteps the sub-device
lottery entirely. The cost is that the two halves are kept in step by scheduling
them back to back rather than by the hardware, so nothing may be inserted
between those two calls — a delay there is a delay between a fill and its key.

**Measured: fill on port 2, key on port 4, a lower third on air for 90 s at
1080p59.94 with zero late frames.**

**Not yet verified: that the key signal is correct on a scope.** The frames are
scheduled and the card is happy, and the key buffer is built as the spec
requires (alpha replicated to R, G and B, opaque, never through a colour
matrix). Whether a downstream keyer produces clean edges from it needs a scope
or a monitor on the key port, which needs the Evertz box at church.

---

## A bind group added to a shared pipeline layout breaks every other pipeline

**2026-08-15, adding images (M3a).** Images need a texture per node rather than
a shared atlas, so they got their own bind group at index 1. Adding it to the
existing pipeline layout — which box, text and unknown all share — made every
one of those draws fail validation:

```
The current set RenderPipeline with 'pmg-scene-box' label expects a
BindGroup to be set at index 1
```

A pipeline layout is a **contract about what every draw using it must bind**,
not a menu. A box has no image to bind and should not have to invent a dummy
texture to satisfy one. There are two pipeline layouts now: the shared one
declares group 0 only, and the image pipeline declares 0 and 1.

Caught by `crates/pmg-render/tests/compositing.rs`, written in M2a for an
entirely different reason — which is the argument for pixel-level tests over
"it looked right when I ran it".

## A 2×2 texture stretched large is almost entirely interpolation

**2026-08-15.** The first image test built a 2×2 PNG — one texel per quadrant —
stretched it to 64×64, and asserted the transparent corner was transparent. It
read `[60, 0, 0, 60]`.

That is bilinear filtering doing exactly its job: with a 2×2 source, only the
four texel centres are pure and every other sample is a blend of all four. The
test was measuring interpolation and calling it a renderer bug.

Fixtures for sampling tests need quadrants **large enough that a sample sits far
from any edge**. The rewritten test uses 32×32 blocks. The same trap is waiting
for anyone testing video scaling later.

---

## The fill+key pair wants PREMULTIPLIED fill, and M2e sent straight

**2026-08-15, found by reading the spec while building M3b.**
`docs/specs/color-and-key.md` §5 states the conventions:

| Backend | Wants |
|---|---|
| NDI (`UYVA`/`RGBA`) | Straight |
| DeckLink 8-bit BGRA embedded alpha | Straight |
| **DeckLink separate fill + key** | **Fill premultiplied over black**, key is the alpha plane |

`pmg-out-decklink` shipped in M2e unpremultiplying in **both** cases. That is
right for embedded alpha and wrong for a pair: a downstream keyer computing
`bg*(1-key) + fill` expects the fill already multiplied by coverage, so straight
fill **brightens every soft edge** — the bright-fringe half of the failure §5
describes.

It survived review because nothing in the code said which convention it was
implementing; it just unpremultiplied because that is what the NDI backend does.
Both branches now name their convention in a comment beside the arithmetic.

Premultiplied-over-black is also *free*: it is exactly what the renderer already
produces, so the pair path now copies rather than dividing.

**Still unverified on a scope** — this is a reasoned correction from the spec,
not a measurement. The soft-edge test pattern over a bright and a dark
background is what settles it, and that needs the Evertz box.

## Colour config is plumbed; the matrix is not yet applied at the boundary

**2026-08-15, M3b.** Per-output `matrix`, `range`, `key_range` and `key_mode`
are stored server-side, travel in the assignment, and land on the node's
`Output`. `pmg-color` implements RGB→YCbCr for 601/709/2020 with correct narrow
spans (luma 16..235, chroma 16..240 — *different*, which is the detail
hand-rolled conversions usually miss) and the key path that bypasses all of it.

**What is NOT yet true: the output backends do not consult it.** Both NDI and
DeckLink are handed 8-bit RGBA/BGRA and the card or the SDK does the YCbCr
conversion with its own assumptions. Applying our matrix means emitting YCbCr
ourselves — a `v210`/`UYVY` path in the shim — which is real work and not done.

So the model is right and tested, the configuration is real and end-to-end, and
the last inch is missing. Recorded here rather than implied complete, because
"the matrix setting exists" and "the matrix setting does anything" are very
different claims to make to someone about to trust a colour pipeline.

---

## The node is authoritative about which outputs EXIST

**2026-08-15, M3c.** A node configured with `key-out` and `wall-out` received an
assignment naming `out-0` — the server's default — and `configure()` **created
it**. The result was a phantom output reporting 59.94 fps and zero late frames
with no render thread behind it.

That is worse than an error, because it looks healthy. An operator reading
`/api/nodes` would see three outputs where two exist and only learn otherwise by
noticing nothing ever appears on the third.

The rule, now enforced: **capabilities are discovered, never configured.** The
node knows its own wiring; the server assigns *layers to an output*, it does not
bring one into being. `LayerEngine::seal_outputs()` is called once the node's own
outputs are in place, after which an assignment naming an unknown output is
logged and ignored rather than obeyed.

The same instinct as `Hello`'s capability block: what the hardware said, not what
someone typed.

## One render thread per output GROUP, not per output

**2026-08-15, M3c.** An output group is a set of outputs sharing a clock
(ADR 0011), and that — not the output count — decides the threading.

A thread per *output* would mean two threads pacing themselves against the same
hardware clock and drifting apart in software for no reason. One thread for
*all* outputs would mean an output on a different reference waiting on a clock it
does not follow. So the threading model is decided by the plant's wiring.

**Render threads must not hold the engine lock while drawing.** Each thread locks
only long enough to snapshot what its outputs should show — an
`Arc<SceneInstance>` plus this frame's timeline values — then releases and draws.
`Armed::instance` became an `Arc` for exactly this. Holding it across a draw
would serialise every group back into one thread wearing several hats.

**Known cost, measured rather than assumed:** arming still happens under that
lock, so a take briefly stalls every group. An arm is 0.3–2 ms against a
16.68 ms budget and takes are a few times a minute, so it costs at most one late
frame on a take. Two outputs across two groups ran at 59.94 with **zero** late
frames through repeated takes. Moving the arm outside the lock is the obvious
next step if it ever shows up on air.

## Reference status must be read through the handle you already hold

**2026-08-15, M3c.** `Sender::reference()` first re-opened the DeckLink device by
index and got `None` every single time — because the device was already open,
**by us**. The Sender now keeps its `Device` handle for the life of the output
and reads through it. (It was previously `std::mem::forget`-ed, which kept the
device alive but threw away the only way to ask it anything.)

Measured on a DeckLink Quad 2 sub-device 1 with no reference generator attached:

```
sync: { present: true, locked: false, format: "" }
```

**What `present: true` means here is genuinely unclear** and should not be
reported to an operator as fact until it is settled. With nothing plugged into
the reference input it may mean "this card has a reference connector" rather
than "a reference signal is detected". Distinguishing them needs a blackburst or
tri-level generator to toggle — the Evertz box at church. Until then the UI shows
the three states the card describes and does not editorialise.

`None` is reserved for backends with **no reference concept at all** (NDI,
headless) and is deliberately distinct from `present: false`. Showing them alike
would tell an operator their card had lost reference when it never had one.

## Ingest converts long-GOP, and says why in terms of the file

**2026-08-15, M3d.** Media arriving at the server is probed and judged into three
outcomes: **accept** all-intra untouched, **convert** anything long-GOP, or
**reject** what has no video at all.

Measured with ffmpeg 7.1.1:

| In | Verdict |
|---|---|
| H.264, 250-frame GOP, yuv420p | converted → ProRes 422 HQ |
| ProRes 4444, `yuva444p12le` | accepted as-is, **alpha intact** |
| a text file | rejected |

Two things this gets right on purpose. **All-intra media is not re-encoded** — a
ProRes master run through ProRes again costs a generation of quality and a lot of
disk for nothing. And **4444 is chosen only when there is alpha**, because it
roughly doubles the file for a channel that does not exist; `pix_fmt` is the only
reliable signal, since containers claim alpha they do not have.

The rejection wording is the point of the feature as much as the conversion is:

> *h264 is long-GOP, which cannot seek to an exact frame — a clip told to start
> on a mark would start near it, after a pause. Converting to ProRes 422 HQ.*

"Unsupported format" sends someone to a forum. That sentence tells them what is
wrong with **their** file and what is about to happen to it.

Ingest is a **job**, not a request: a service-length clip takes minutes, and an
HTTP request held open that long is one something in the middle will drop.
Conversions run one at a time, because parallel transcodes on a booth machine
would compete with whatever is going on air.

## ffmpeg as a child process, because the local build is GPL

**2026-08-15, M3d.** `docs/RISKS.md` R5 recommends `rsmpeg`/`ffmpeg-next` — FFI
into libavcodec. Playback ships as a **child process feeding raw frames over a
pipe** instead, and the deciding reason is not performance.

The ffmpeg on this machine is Homebrew 7.1.1, configured `--enable-gpl
--enable-version3`. That is the default build, and it is what any developer on
this project will have. Linking it in-process makes the whole binary GPL. R5
says "LGPL components only, dynamically linked, no GPL" and ADR 0003 keeps that
as a productization seam — **a seam that closes silently, at build time, on a
developer's own machine is not a seam.** Invoking a separate program over a pipe
is the ordinary aggregation boundary and closes nothing.

Performance turned out not to argue the other way. 1080p, 1200 frames, Apple
M-series, otherwise idle, frames written to `/dev/null`:

| Source | Through the pipe | vs 59.94 |
|---|---|---|
| ProRes 422 HQ, no alpha | 736 fps | 12.3× |
| ProRes 4444, alpha, premultiplied by ffmpeg | 168 fps | **2.8×** |
| seek + one frame, cold spawn | 80 ms | — |

2.8× on the case that matters — keyed video — is enough and is not enormous. It
cost about **9.4 cores** to get, and that is the number to watch: it is decode
competing with a render loop on the same box, not a pipe bottleneck. Capping
`-threads 4` made it *slower* (126 fps) for barely less total CPU, so there is no
cheap win there.

Revisit FFI when a node must play several 4K clips at once, or when a frame needs
to reach the GPU without a trip through system RAM. Neither is true yet, and
starting here makes neither harder.

Two details that are load-bearing rather than incidental:

- **`-stream_loop -1` for looping, never re-spawning at EOF.** A respawn puts
  that 80 ms hole in every loop point. Letting ffmpeg loop internally also keeps
  the emitted frame index monotonic, which is what lets the clock stay a single
  multiply instead of a modulo with a wrap case.
- **Premultiply with ffmpeg's own filter, and only when there is alpha.**
  `format=rgba,premultiply=inplace=1` runs off our thread and vectorised. It also
  costs a full pass over every pixel, so applying it to opaque media buys a
  no-op at 2.8× the price of the decode.

## A missing pre-roll looks exactly like a decoder underrun

**2026-08-15, M3d.** The first playback measurement reported 6 held and 12
dropped frames over 10 s of a 36 MB keyed clip. Held frames mean the decoder fell
behind and the output repeated a picture — and on a clip that small, against a
decoder measured at 2.8× realtime, that made no sense.

It was not an underrun. **A deeper queue changed nothing**: depths 3, 8 and 24
gave byte-identical counts (6 held, 12 dropped), while depth 1 gave 53. Real
starvation improves with buffer; a deterministic result does not.

The cause was that the clock started before the decoder did. ffmpeg needs ~120 ms
to spawn and produce its first frame, so output frame 0 asked for a picture that
would not exist until output frame 7. From then on the player was permanently
seven frames behind the wall clock and spent the next stretch sprinting to catch
up — holding and discarding pictures the whole way. **The stutter was at the
start and the symptom was spread across the clip.**

Waiting for the first frame before starting the clock — `VideoPlayer::preroll`,
called at arm — takes it to **0 held, 0 dropped** on the same clip, and to 0 held
with a single expected pulldown drop on a pathological 225 MB/s 4444 clip. Worst
case pulling a frame on the render thread: **0.085 ms against a 16.68 ms budget**,
because all the work is on the decoder's own thread.

This is the arm/take split earning its keep for the third time, after layout and
shaping. A clip is armed before it is taken; the 120 ms is spent off air, where
there is nothing to be late for. The general form is worth keeping in mind:
**a fixed cost at startup does not show up as a startup problem — it shows up
smeared across everything that follows, wearing the costume of a capacity
problem.**

## Video is an image whose texture changes, and that was the whole renderer change

**2026-08-15, M3d.** Playback needed exactly one new method in `pmg-render`:
`SceneRenderer::update_image`. A clip draws through the image pipeline, with
`fit` resolved to a `uv` rectangle at build time exactly as a still's is; the
only difference is that something overwrites the texture between frames.

What that bought: no new shader, no new pipeline, no new bind group layout, and
no new way for a clip to differ from a still in how it composites, clips, or
animates. The pure set still builds for `wasm32` unchanged, because a queue write
is not I/O — and `pmg-media`'s video module is compiled out on wasm entirely, so
the still decoder stays portable while the video path is frankly native.

The one thing this required in the renderer was **keeping the `wgpu::Texture`
handles**, not just their bind groups. A bind group gives no way to write through
it.

Measured, 1080p ProRes 4444 with alpha, looping under a lower third, 21 s at
59.94 on an M3 Max:

| | Worst frame | Over budget |
|---|---|---|
| with the clip | 15.17 ms | 0 of 1272 |
| the same template, no clip | 12.47 ms | 0 of 1272 |

So the per-frame texture upload costs about **2.7 ms of headroom** at 1080p,
leaving ~1.5 ms of margin against 16.68 ms. That is the number to watch when a
second clip or a 4K output arrives — it is also the measurement that would
justify getting frames to the GPU without the trip through system RAM.

**Two harness mistakes worth not repeating.** `pmgctl take` runs its render loop
flat out, which is the point when measuring render cost — but asking a player for
frames at 280 fps while its decoder produces 170 reported 247 held frames out of
253, none of them real. It now paces to the output rate whenever a clip is bound,
because playback is only measurable in real time. And the first decode test built
its fixture from `testsrc2` at 64x48 and asserted consecutive frames differed;
they did not, and the test failed while the decoder was working perfectly. The
fixture now makes red a function of the frame number, so "advanced" and "stuck"
are distinguishable by construction.

## A clip starts up to 120 ms after the take, and holds its first frame until it does

**2026-08-15, M3d.** `CueOp::Arm` still builds an instance and throws it away —
there is no preview slot yet, and that is honest rather than accidental. So a
clip's decoder is opened at **take**, not at arm, and ffmpeg's ~120 ms to first
frame lands on air.

What the operator sees is the clip's **first frame**, held, for about seven
frames at 59.94. Not black, and not an empty layer: the poster frame is decoded
at assignment time and is already in the texture before the take. The clock then
starts when the first decoded frame actually arrives, so nothing is skipped — the
clip plays from frame 0 in full, it simply begins a fifth of a second late.

Starting the clock at the take instead would be worse in a way that is easy to
miss: the player would be seven frames behind immediately and would discard those
seven to catch up, so the clip would *begin a fifth of a second in* having frozen
to get there. Same wait, less clip.

**The fix is real pre-arm** — retaining the armed occupant so its decoder
pre-rolls before the take — and it wants the preview slot that does not exist
yet. Recorded here rather than worked around, because the workaround (opening
decoders at assignment) would leave an ffmpeg running for every clip in the show.

## An assignment says a hash, never what kind of file it is

**2026-08-15, M3d.** The node decides what an asset *is* by reading its first four
bytes, not by reading it and seeing what decodes. That distinction stopped being
cosmetic the moment clips existed: the previous code did `cas.get(&hash)` — the
whole file into memory — and tried `decode_image` on it. For a service-length
ProRes clip that is gigabytes of RAM spent to discover the file is not a PNG,
which would undo the entire reason video is decoded a frame at a time.

Same reasoning as `looks_like_a_font`, which was already there for the same
reason: a CAS filename is a hash, so there is no extension to trust, and trusting
one would be worse than having none.

## DeckLink audio needs a preroll bracket, and says E_ACCESSDENIED without one
*2026-08-15 · MacBook Pro arm64 · Desktop Video 16.3 · DeckLink SDK 16.0 · DeckLink Quad 2, sub-device 1*

`EnableAudioOutput` succeeding does not mean the stream will take a sample. The
first `ScheduleAudioSamples` returns **`0x80000009` (E_ACCESSDENIED)** — the
stream exists and is not open for business.

Continuous audio wants an explicit bracket, and the video path needs no
equivalent, which is exactly why it is easy to miss:

```
EnableVideoOutput → EnableAudioOutput → BeginAudioPreroll
  → schedule the preroll frames, video AND audio
→ EndAudioPreroll → StartScheduledPlayback
```

`EndAudioPreroll` must come **before** `StartScheduledPlayback`. Starting with
the stream still prerolling leaves the card holding samples it will not play,
and the video runs away from audio that never starts.

## The card's own audio buffer is the instrument, and it is the only one

**2026-08-15, M3e.** Embedded audio has no error to check. The API accepts a
wrong sample count without complaint, the video counters stay perfect, and the
fault is a slow walk out of lip sync. What is actually observable is
`GetBufferedAudioSampleFrameCount`: audio runs continuous, the card drains it at
its own rate, and we top it up once per video frame — **so if the cadence is
right, the level is flat.**

Two minutes at 1080p59.94, 2 channels, 7193 frames, on the same run:

| Cadence fed | abuf @30 s | @60 s | @90 s | Late | Dropped |
|---|---|---|---|---|---|
| exact (801/801/801/801/800) | 5550–5863 | 5534–6098 | 5460–6080 | 0 | 0 |
| **flat 800, on purpose** | 3954–4522 | 2543–3134 | 1111–1730 | 0 | 0 |

The exact cadence sits in the same band for the whole run. The naive one loses
**0.79 samples a frame measured** against 0.8 predicted, and would have hit zero
and clicked at about two minutes. Over a two-hour service that is **7.1 seconds**
of lip sync.

Note the last two columns. **Zero late and zero dropped in both rows.** Every
video counter reads perfect while the audio quietly empties, which is the whole
reason this needed an instrument rather than an assertion — and why `pmgctl
decklink out --audio-naive` exists at all. A measurement you cannot make fail on
demand is not yet evidence.

## A loop that retries every error turns a fault into a hang

**2026-08-15, M3e.** The soak's scheduling loop treated *any* `Err` as
backpressure: back off a quarter frame and try again. That was correct when
`BUSY` was the only error it could see. The moment audio could fail, the loop
spun on E_ACCESSDENIED for **seven minutes**, scheduling zero frames, printing
nothing, and looking exactly like a slow card.

Two fixes, and the second matters more than the first. Only `PMG_DL_ERR_BUSY` is
backpressure now; everything else stops and says so. And the shim records the
**vendor's own HRESULT and which call produced it**, because
`PMG_DL_ERR_INTERNAL` says "the SDK said no" and `ScheduleAudioSamples returned
0x80000009` says what to go and read. Flattening a vendor error code into a
generic one throws away the only part with diagnostic value.

## A clip with sound, on air, out of SDI — and the four bugs the unit tests could not see
*2026-08-15 · MacBook Pro arm64 · Desktop Video 16.3 · DeckLink Quad 2 sub-device 1 · M3e*

The end-to-end M3e was missing: ingest → publish → assign → stage → take → SDI,
with embedded audio. It passes. `tools/e2e-clip-audio.sh` runs it.

```
attempt 1: {"result":"applied","fired_at_frame":79,...}
Fullscreen -> Reel with sound | phase hold | degraded False
audio_buffered 5829..6015 · peak_dbfs -21.1 · late 0     (60 s)
```

Buffer flat, zero late frames, and the peak matches the source **exactly**:
measured straight off the file through the same conversion, `source peak
189792256 = -21.1 dBFS`. The path neither attenuates nor silences.

**Every step of getting there found something a unit test could not.**

**1. `http://ws://127.0.0.1:8519/cas/…`** — the node prefixed `http://` onto a
`--server` that already carried a scheme. The symptom is the instructive part: it
linked, reported healthy, acked its assignment, and simply never became ready,
because **an un-armable Item is a designed state and not an error**. It looked
exactly like a node with nothing to play. Fixed with `http_base`, which also
maps `wss://` to `https://` rather than silently downgrading asset transfer.

**2. The node never checked whether a cue was late.** `apply(..., received_late)`
was called with a hard-coded `false`. The rejection branch, its reason code, and
a passing test for it all existed — on the **engine**. The runtime that decides
when to take that branch had no test at all, so *"reject a late take, never fire
it"* was true of `LayerEngine` and false of `pmg-node`.

It surfaced as the worst possible shape: the server stopped waiting at 750 ms
while the node was busy staging a 566 MB clip, returned `503 cue_not_acked` — and
the node then applied the take anyway when it got round to it. **The operator was
told the take failed and the graphic went on air.** That is the dangerous
direction of the lie ADR 0008 exists to prevent.

Lateness is now measured from when the frame came **off the socket**, not from
when it is used, because the gap between those two is exactly the delay a busy
node introduces and cannot otherwise see. Network transit is still not covered —
that needs an `issued_at` on the wire, and the server's clock is not ours to
trust. Worth doing separately.

The general lesson: **a unit test on the callee proves the callee.** A rule this
load-bearing needed a test on the caller, and now has one (`is_late`).

**3. A flat audio buffer proves the cadence, not the content.** Feeding exactly
the right number of *zeroes* produces an identical graph. The node logs peak
dBFS alongside the buffer depth for that reason — the difference between "audio
is timed correctly" and "audio is there".

**4. The harness lied three times before it told the truth**, and all three were
the same mistake — suppressing errors to make it robust:

| What | Hidden by |
|---|---|
| `n[0]["id"]` where the field is `node_id` | `2>/dev/null \|\| true` on the poll |
| publish rejected: manifest carries its own `template_id` | `curl -sf`, which discards the body |
| a stale server on 8519, so the node linked to the wrong one | no check that the server we started is the one answering |

Each looked like a timeout. `curl -f` and `|| true` are how a failing step
becomes a slow one.

## NDI audio: two structs one letter apart, and only one of them is the send shape
*2026-08-15 · NDI runtime 6.3.2.0 (Apple) · M3e*

NDI carries **planar float**; everything upstream here is interleaved 32-bit
integer. Three things about closing that gap were not guessable.

**`NDIlib_util_send_send_audio_interleaved_32s` does not exist.** `nm` on the
6.3.2.0 runtime exports the `from_interleaved` converters and
`send_send_audio_v2`, and no combined helper. So the path is convert, then send.
The conversion is left to the SDK rather than written here, because it also
applies NDI's reference-level convention — and picking that wrong is a feed
20 dB hot or quiet with nothing reporting an error. `reference_level: 0` is
correct for material already at the level it is meant to be: measured, a tone
sent at 0.25 full scale came back at **−12.0 dBFS** against −12.04 expected.

**`NDIlib_recv_capture_v3` takes an audio *v3* frame, not v2.** They differ:

| | v2 (send) | v3 (receive) |
|---|---|---|
| `timecode` | 16 | 16 |
| `FourCC` | — | **24** |
| `p_data` | 24 | 32 |
| `channel_stride_in_bytes` | 32 | 40 |
| size | 56 | 64 |

Passing a v2 where v3 is expected misreads **every field after the timecode** —
a pointer read as an integer, a stride read as a pointer. It would not crash
where the mistake was; it would report a plausible rate and then hand back
garbage. Both are now asserted by offset, plus an assertion that their sizes
**differ**, so a refactor that accidentally unifies them fails loudly.

The two coexist because the send path genuinely wants v2:
`send_send_audio_v2` and `util_audio_from_interleaved_32s_v2` are both defined
in terms of it. Same concept, two layouts, one letter apart.

**Verified by sending and receiving on one machine**, which is the same
discipline `pmgctl ndi probe` was built with for video — decode real fields off
a real frame rather than trust the struct:

```
audio: 48000 Hz, 2 ch, 801 samples, stride 3232 B, peak -12.0 dBFS
```

801 is the exact per-frame count `pmg_audio::AudioClock` produces at 59.94, so
the cadence survives the wire. Note the receive stride (3232 B = 808 floats)
is **not** the send stride (801 × 4 = 3204): NDI repacks, and a reader that
assumed `no_samples * 4` would walk off by seven samples a channel per frame.

## WebCodecs preview: H.264 over a WebSocket, and a VideoFrame that lies after close()
*2026-08-15 · Chrome via the in-app browser · M3f*

MJPEG sent every frame whole. H.264 sends the difference: a static 480x270
preview measured **0.05 Mbit/s** against MJPEG's several, and a moving one stays
inside a 4 Mbit budget. Verified by decoding it in a real browser — 103 frames
on a canvas, showing the checkerboard, which is the correct picture when nothing
is on air.

Stream shape, confirmed by reading the NALs off the wire:

```
msg 1: key=true  2977 B  NALs [7,8,6,5]   SPS, PPS, SEI, IDR
msg 2: key=false   36 B  NALs [1]         P slice
```

Four things that had to be right, none of them guessable:

- **SPS/PPS before every keyframe**, via `-bsf:v dump_extra=freq=keyframe`.
  Without it a browser that connects one second in has no parameter sets and
  shows nothing at all, forever, with no error anywhere.
- **The server caches the last keyframe** and sends it on connect, so a joiner
  paints in **4 ms** rather than waiting up to a GOP.
- **Annex-B, not AVCC.** Configuring `VideoDecoder` with no `description` selects
  Annex-B, which is what `-f h264` writes. AVCC would mean parsing SPS/PPS in
  TypeScript to build an `avcC` — a second implementation of something that must
  not disagree.
- **A one-byte key/delta header per message.** `EncodedVideoChunk` needs the
  type, and a decoder cannot start on a delta; dropping deltas until the first
  key is what makes a mid-service connection paint instead of throw.

**The trap: a `VideoFrame` reads as 0x0 after `close()`.** The test page reported
`LIVE 0x0` while painting a perfectly good picture, because it read
`displayWidth` in the same statement that had already closed the frame. Closing
zeroes the metadata rather than throwing. Read the dimensions **before**
`close()` — and `close()` must still always happen, because a leaked frame holds
a GPU buffer and stalls the decoder within seconds.

MJPEG is kept behind `--preview-mjpeg`. It needs no decoder in the browser at
all, which makes it the instrument that answers "is ANY picture coming out of
this node" when the answer for H.264 is no.

## A two-hour soak: one hiccup, and an audio offset that never recovers
*2026-08-16 · MacBook Pro arm64 · Desktop Video 16.3 · DeckLink Quad 2 · M3*

Two hours at 1080p59.94, a 30 s clip looping on Fullscreen with embedded audio,
446 lower-third take/take-out cycles on another layer. **894 cues, every one
applied, no warnings and no errors in the whole run.** And it failed three
checks — all of them the same five seconds.

```
  render over budget    17
  card displayed late   55        FAIL
  clip held frames      108       FAIL
  audio buffer trend    5976 -> 14063 samples (+168.5 ms)   FAIL
  audio peak            -21.1 dBFS  (programme)
```

The three are one incident. At **frame ~45,000 — twelve and a half minutes in —**
inside a single 300-frame window:

| | before | after |
|---|---|---|
| clip held frames | 0 | 108 |
| card displayed late | 0 | 53 |
| audio buffered | 4,241 | 14,050 |

Then **nothing else happened for an hour and forty-seven minutes.** Held stayed
at 108, late crept to 55 and stopped, and the buffer sat dead flat at ~14,000.
Whatever stalled the machine for a second at 12.5 minutes stalled it once.

**The finding is not the hiccup. It is that the hiccup is permanent.**

The audio buffer went from ~5,900 samples (123 ms) to ~14,000 (292 ms) and
**stayed there for the rest of the run**. That is a **+168 ms lip-sync offset
that nothing corrects and nothing reports.** Audio is scheduled continuous: the
card consumes it against its own 48 kHz clock while we top it up once per
*scheduled* frame. When the output stalls, our loop keeps scheduling into
DeckLink's queue, audio piles up ahead of picture — and once ahead, it is ahead
forever. There is no feedback path.

Every other counter recovered. Held frames stopped climbing; late frames stopped
climbing; the frame budget was met for the next 1.75 hours. **Only the sync
error is cumulative, and it is the only one a viewer would still be noticing two
hours later.**

The fix is to close the loop on the instrument that already exists.
`GetBufferedAudioSampleFrameCount` is read every five seconds for exactly this;
it needs to *drive* the sample count rather than only report it — hold a target
level and correct gently toward it, which is what playout hardware does and what
`pmg_audio::AudioClock` deliberately does not do on its own. The exact cadence
stays right; a slow correction rides on top.

**What the soak proves about soaking.** Three minutes passed cleanly, twice.
The stall at 12.5 minutes and the permanence of its consequence are both
invisible at that length — and the permanence is the part that matters, because
a booth machine will hiccup and the question is only whether it recovers.
