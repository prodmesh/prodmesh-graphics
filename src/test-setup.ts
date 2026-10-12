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

/**
 * Follow mode prefers the cue socket and falls back to `POST /api/cue`. jsdom
 * HAS a WebSocket, and would dial the test origin, fail asynchronously, and
 * only then fall back — so every Follow test would race a connection error.
 * Tests get the HTTP path unless they stub a socket to test the socket.
 */
// @ts-expect-error — deliberately absent, which is what the code checks for.
delete globalThis.WebSocket
