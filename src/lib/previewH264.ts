/**
 * The H.264 preview channel, decoded with WebCodecs.
 *
 * The node sends one access unit per WebSocket binary message, prefixed with a
 * single byte: 1 for a keyframe, 0 otherwise. `VideoDecoder` needs to be told
 * which is which, and it cannot start on anything but a key.
 *
 * # Annex-B, deliberately
 *
 * Configuring with no `description` puts `VideoDecoder` in Annex-B mode, which
 * is what ffmpeg's `-f h264` writes. The alternative — AVCC plus an
 * `avcC` description — means parsing SPS/PPS in TypeScript to build the
 * description, and getting a second implementation of something that must not
 * disagree. See ADR 0004's reasoning about the scene format, applied smaller.
 *
 * # Failure is a sentence, not a blank rectangle
 *
 * Every path here reports through `onStatus`, because a preview that silently
 * shows nothing is indistinguishable from a node that is not rendering — and
 * those need completely different responses from an operator.
 */

export type PreviewStatus =
  | { state: 'connecting' }
  | { state: 'waiting-for-key' }
  | { state: 'live'; width: number; height: number }
  | { state: 'unsupported'; detail: string }
  | { state: 'failed'; detail: string }

export type PreviewHandle = { close: () => void }

/** `ws://host:port/` from the node's advertised `http://host:port/`. */
export function previewSocketUrl(httpUrl: string): string {
  return httpUrl.replace(/^http/, 'ws')
}

export function startH264Preview(
  url: string,
  canvas: HTMLCanvasElement,
  onStatus: (s: PreviewStatus) => void,
): PreviewHandle {
  // Named rather than assumed: a browser without WebCodecs should say so, not
  // present an empty box. Safari gained VideoDecoder late and this is exactly
  // the machine someone will try it on.
  if (typeof window === 'undefined' || !('VideoDecoder' in window)) {
    onStatus({
      state: 'unsupported',
      detail: 'This browser has no WebCodecs. Run the node with --preview-mjpeg.',
    })
    return { close: () => {} }
  }

  const ctx = canvas.getContext('2d')
  let closed = false
  let sawKey = false
  let decoder: VideoDecoder | null = null

  onStatus({ state: 'connecting' })
  const ws = new WebSocket(url)
  ws.binaryType = 'arraybuffer'

  const fail = (detail: string) => {
    if (!closed) onStatus({ state: 'failed', detail })
  }

  ws.addEventListener('open', () => {
    if (closed) return
    onStatus({ state: 'waiting-for-key' })
    decoder = new VideoDecoder({
      output: (frame) => {
        try {
          if (canvas.width !== frame.displayWidth || canvas.height !== frame.displayHeight) {
            canvas.width = frame.displayWidth
            canvas.height = frame.displayHeight
            onStatus({ state: 'live', width: frame.displayWidth, height: frame.displayHeight })
          }
          ctx?.drawImage(frame, 0, 0)
        } finally {
          // Always. A VideoFrame holds a GPU buffer, and leaking these stalls
          // the decoder within a few seconds rather than growing slowly.
          frame.close()
        }
      },
      error: (e) => fail(e.message),
    })
    // Baseline 3.1 — the profile every decoder has. The node encodes whatever
    // its hardware prefers; if that ever stops matching, it shows up here as a
    // configure error rather than as a picture that is subtly wrong.
    decoder.configure({ codec: 'avc1.42E01F', optimizeForLatency: true })
  })

  ws.addEventListener('message', (ev) => {
    if (closed || !decoder || decoder.state !== 'configured') return
    const buf = new Uint8Array(ev.data as ArrayBuffer)
    if (buf.length < 2) return
    const key = buf[0] === 1
    // A decoder cannot start mid-GOP. Dropping deltas until the first key is
    // what makes a mid-service connection paint rather than throw.
    if (!key && !sawKey) return
    sawKey = true
    try {
      decoder.decode(
        new EncodedVideoChunk({
          type: key ? 'key' : 'delta',
          // Microseconds. The node paces the stream; this only has to be
          // monotonic for the decoder's own ordering.
          timestamp: performance.now() * 1000,
          data: buf.subarray(1),
        }),
      )
    } catch (e) {
      fail(e instanceof Error ? e.message : String(e))
    }
  })

  ws.addEventListener('error', () => fail('The preview connection failed.'))
  ws.addEventListener('close', () => {
    if (!closed) onStatus({ state: 'failed', detail: 'The node closed the preview connection.' })
  })

  return {
    close: () => {
      closed = true
      try {
        ws.close()
      } catch {
        // Already gone.
      }
      if (decoder && decoder.state !== 'closed') decoder.close()
    },
  }
}
