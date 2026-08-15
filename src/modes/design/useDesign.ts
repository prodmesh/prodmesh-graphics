/**
 * The bridge from React to the Rust renderer running in this tab.
 *
 * # Why the WASM module is loaded lazily and once
 *
 * It is ~2.3 MB and it asks the browser for a GPU device. Neither belongs on
 * the critical path of an operator opening Show — Design is the only mode that
 * needs it, and a booth machine that never opens Design should never pay for it.
 */

import { useCallback, useEffect, useRef, useState } from 'react'

// The declarations come from wasm-bindgen, generated alongside the .wasm by
// `npm run wasm`. Hand-writing them would let this file and the Rust drift
// apart silently, which is the one failure the shared-code premise cannot
// afford.
type WasmModule = typeof import('../../wasm/pmg_wasm.js')
// Awaited<ReturnType<...create>> rather than InstanceType: wasm-bindgen marks
// the constructor private (instances only ever come from `create`), and
// InstanceType cannot name a class with a private constructor.
export type DesignSession = Awaited<ReturnType<WasmModule['DesignSession']['create']>>

let modulePromise: Promise<WasmModule> | null = null

async function loadWasm(): Promise<WasmModule> {
  if (!modulePromise) {
    modulePromise = (async () => {
      const mod = await import('../../wasm/pmg_wasm.js')
      await mod.default()
      return mod
    })()
  }
  return modulePromise
}

export type Compiled = { template_rev: string; scene: Uint8Array; manifest: string }

/**
 * Compile a template document to canonical CBOR and its manifest.
 *
 * The `template_rev` this returns is **byte-identical** to the one `pmgctl`
 * produces for the same document — same encoder, same types, compiled twice
 * (ADR 0001). That is what lets the browser publish a revision by id and a node
 * fetch it by the same id with nothing in between having to agree.
 */
export async function compileTemplate(doc: unknown): Promise<Compiled> {
  const wasm = await loadWasm()
  return wasm.compile_template(JSON.stringify(doc))
}

type Status = 'idle' | 'loading' | 'ready' | 'error'

/**
 * Arm a template and paint it into a canvas.
 *
 * Arming is expensive and happens when the document or data changes. Painting a
 * frame is cheap and happens on every scrub — the same split the node lives by,
 * and the reason dragging the timeline is smooth rather than a slideshow.
 */
export function useDesignCanvas(
  canvasRef: React.RefObject<HTMLCanvasElement | null>,
  doc: unknown,
  data: Record<string, string>,
  fonts: Uint8Array[],
) {
  const [status, setStatus] = useState<Status>('idle')
  const [problem, setProblem] = useState<string | null>(null)
  const [info, setInfo] = useState<{ degraded: boolean; omitted: string[] } | null>(null)
  const session = useRef<DesignSession | null>(null)
  const painting = useRef(false)

  const paint = useCallback(async (timeline: string | undefined, tMs: number) => {
    const s = session.current
    const canvas = canvasRef.current
    if (!s || !canvas) return
    // Drop a frame rather than queue one. A scrub fires far faster than a GPU
    // round trip, and a queue would render every intermediate position long
    // after the pointer has moved on.
    if (painting.current) return
    painting.current = true
    try {
      const rgba = await s.renderFrame(timeline, tMs)
      const ctx = canvas.getContext('2d')
      if (!ctx) return
      // putImageData, never drawImage-through-a-data-URL: the bytes go to the
      // screen untouched by colour management. It is also the same readback the
      // parity harness compares, so what is measured is what is shown.
      ctx.putImageData(new ImageData(new Uint8ClampedArray(rgba), s.width, s.height), 0, 0)
    } catch (e) {
      setProblem(String(e))
    } finally {
      painting.current = false
    }
  }, [canvasRef])

  useEffect(() => {
    let cancelled = false
    if (!doc || fonts.length === 0) {
      setStatus('idle')
      return
    }
    setStatus('loading')
    setProblem(null)

    ;(async () => {
      try {
        const wasm = await loadWasm()
        const s = await wasm.DesignSession.create(JSON.stringify(doc), JSON.stringify(data), fonts)
        if (cancelled) {
          s.free()
          return
        }
        session.current?.free()
        session.current = s
        setInfo({ degraded: s.degraded, omitted: [...s.omitted] })

        const canvas = canvasRef.current
        if (canvas) {
          canvas.width = s.width
          canvas.height = s.height
        }
        setStatus('ready')
        await paint(undefined, 0)
      } catch (e) {
        if (!cancelled) {
          // Compile and layout failures land here, and they are the designer's
          // own mistakes — a bad field reference, a template that will not
          // solve. Show the message rather than a blank canvas.
          setProblem(e instanceof Error ? e.message : String(e))
          setStatus('error')
        }
      }
    })()

    return () => { cancelled = true }
  }, [doc, data, fonts, canvasRef, paint])

  useEffect(() => () => { session.current?.free(); session.current = null }, [])

  return { status, problem, info, paint }
}
