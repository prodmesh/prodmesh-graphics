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

*Empty. Nothing in this repository has touched hardware yet — see
`docs/STATE.md`.*

The first entries will come from the M0 spikes:

- **S3** will produce the DeckLink facts: which models on hand support key+fill
  on two ports of one device, what the completion callback's actual threading
  looks like, what preroll depth is needed to avoid late frames at 1080p59.94,
  and which `bmdOutputFrameCompletionResult` values show up in practice.
- **S1** will produce the render-parity numbers for this GPU and this browser
  version, which is the only form that claim can honestly take.
- **S6** will produce the WebGPU-in-the-booth numbers: init time, bundle size,
  and frame time on the actual booth Macs rather than a dev laptop.

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
