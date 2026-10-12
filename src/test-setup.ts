/**
 * jsdom has no EventSource, and `lib/stream.ts` opens one 30 ms after the first
 * subscriber mounts. A test that stays mounted longer than that — which a CI
 * runner does and a fast laptop does not — threw an unhandled ReferenceError
 * after every assertion had passed, and failed the run (Edit.test.tsx, from
 * 2026-08-22 until this). Tests that care what the stream delivers stub their
 * own with `vi.stubGlobal`; everything else gets one that never speaks.
 */

class InertEventSource {
  readonly readyState = 0
  constructor(readonly url: string) {}
  addEventListener() {}
  removeEventListener() {}
  close() {}
}

if (typeof globalThis.EventSource === 'undefined') {
  globalThis.EventSource = InertEventSource as unknown as typeof EventSource
}
