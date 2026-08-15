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
