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
 * The interaction shapes (ADR 0019), decoded from the session's JSON answers.
 *
 * Coordinates are CANVAS coordinates — design-space pixels. The pointer layer
 * scales from client pixels once, at the edge; everything inside speaks the
 * canvas's own units, which is also what makes the SVG overlay trivial: its
 * viewBox IS the canvas.
 */
export type Handles = {
  id: string
  /** tl, tr, br, bl — through the node's world transform, so a rotated node
   *  gets a rotated outline rather than a lying axis-aligned one. */
  corners: [[number, number], [number, number], [number, number], [number, number]]
  bbox: [number, number, number, number]
  rotated: boolean
}

export type Guide = { axis: 0 | 1; at: number }

/** serde-encoded `pmg_anim::Ease` — the wire speaks the document's names. */
export type Ease = 'linear' | { cubic: [number, number, number, number] } | { steps: number }

export type DocKey = { t: number; v: number; ease?: Ease }
export type DocTrack = { target: string; keys: DocKey[] }
export type DocTimeline = { name: string; duration_ms: number; loop?: boolean; tracks: DocTrack[] }

/** The AUTHORED facts for one node — what the inspector edits, as distinct
 *  from the solved geometry `Handles` reports. Straight from the document. */
export type NodeFacts = {
  id: string
  /** Encoded straight RGBA in 0..1, or null for an unfilled (structural) box.
   *  For a text node this is the text colour. */
  fill: [number, number, number, number] | null
  radius: number
  opacity: number
  absolute: boolean
  kind: string
  /** Contents masked to this node's bounds. */
  clip: boolean
  /** How each axis is authored: 'px' (fixed) or 'content' (hugs contents).
   *  The solved size is the handles' answer; this is the setting. */
  width_mode: string
  height_mode: string
  /** Authored spacing, [top, right, bottom, left]. */
  margin: [number, number, number, number]
  padding: [number, number, number, number]
  /** Present exactly when the node is text. */
  text?: { expr: string; family: string; weight: number; size: number; align: string; token?: string }
  /** Present exactly when the node draws a picture. */
  image?: { asset_key: string; hash: string | null; fit: string }
  /** Present exactly when the node plays a clip. */
  video?: { field: string | null; fit: string; looping: boolean }
  /** The outline, when there is one: encoded straight RGBA plus width. */
  fill_token?: string
  stroke?: { color: [number, number, number, number]; width: number; token?: string }
  /** The drop shadow, when there is one. */
  shadow?: { color: [number, number, number, number]; dx: number; dy: number; blur: number; token?: string }
  gradient?: { angle: number; stops: [[number, number, number, number], number][] }
}

export type DragResult = {
  /** The ghost to draw. Nothing has been edited yet. */
  rect: [number, number, number, number]
  dx: number
  dy: number
  w: number
  h: number
  guides: Guide[]
}

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

/**
 * The document a "New template" starts from: a canvas-sized transparent root
 * and nothing else. Built by the format crate through wasm — the browser never
 * authors scene JSON, not even an empty one (ADR 0004).
 */
export async function blankDoc(
  id: string,
  name: string,
  width: number,
  height: number,
): Promise<Record<string, unknown>> {
  const wasm = await loadWasm()
  return JSON.parse(wasm.blankDoc(id, name, width, height)) as Record<string, unknown>
}

/**
 * Walk a document forward to the current format version (spec §9.3). Design
 * runs this on every open, so the editors always work on a current-shape
 * document — publishing the result is still the designer's explicit act.
 */
export async function migrateDoc(doc: Record<string, unknown>): Promise<Record<string, unknown>> {
  const wasm = await loadWasm()
  return JSON.parse(wasm.migrateDoc(JSON.stringify(doc))) as Record<string, unknown>
}

/** A theme's content address and canonical bytes, from the same Rust encoder
 *  behind `template_rev` — what a theme save sends the server (ADR 0021). */
export async function themeHash(theme: {
  name: string
  tokens: Record<string, unknown>
}): Promise<{ hash: string; bytes: Uint8Array }> {
  const wasm = await loadWasm()
  return wasm.themeHash(JSON.stringify(theme)) as { hash: string; bytes: Uint8Array }
}

/** Re-resolve a document's tokens from a theme without arming a session —
 *  the apply sweep walks every follower this way. */
export async function rethemeDoc(
  doc: Record<string, unknown>,
  themeJson: string,
  hash: string,
): Promise<Record<string, unknown>> {
  const wasm = await loadWasm()
  return JSON.parse(wasm.rethemeDoc(JSON.stringify(doc), themeJson, hash)) as Record<string, unknown>
}

/** What one uploaded face is: its content hash, and the name and weight the
 *  file itself states. What a font picker offers must be the file's own name,
 *  not a string somebody typed near it. */
export type FaceInfo = { hash: string; family: string; weight: number }

/** Read a font file's own family and weight, via the same parser that shapes
 *  with it. Null for bytes that are not a font. */
export async function fontFacts(bytes: Uint8Array): Promise<{ family: string; weight: number } | null> {
  const wasm = await loadWasm()
  return wasm.fontFacts(bytes) as { family: string; weight: number } | null
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
  /** Image bytes by content hash — the assets the document references,
   *  fetched from the CAS. Arrive late without harm: the canvas shows the
   *  degraded placeholder until the fetch lands and re-arms. */
  images: Record<string, Uint8Array>,
) {
  const [status, setStatus] = useState<Status>('idle')
  const [problem, setProblem] = useState<string | null>(null)
  const [info, setInfo] = useState<{ degraded: boolean; omitted: string[] } | null>(null)
  const session = useRef<DesignSession | null>(null)
  const painting = useRef(false)
  /**
   * Bumped every time `session.current` is REPLACED. The ref mutation is
   * invisible to React, and `status` cannot stand in for it: with a warm GPU
   * adapter the loading→ready flip can land inside one commit, so a consumer
   * keyed on status re-runs with the OLD session and then never again —
   * selection chrome frozen at the pre-edit geometry, but only when the
   * machine is fast, which is the worst kind of bug to chase. Consumers of
   * interaction geometry key on THIS.
   */
  const [generation, setGeneration] = useState(0)

  // The view a paint request carries while another is in flight. Latest wins:
  // a scrub fires far faster than a GPU round trip, and rendering every
  // intermediate position would trail the pointer — but dropping the LAST
  // request entirely would leave the canvas showing a stale frame after the
  // flood ends, which is worse.
  const queued = useRef<[string | undefined, number] | null>(null)

  const paint = useCallback(async (timeline: string | undefined, tMs: number) => {
    const s = session.current
    const canvas = canvasRef.current
    if (!s || !canvas) return
    if (painting.current) {
      queued.current = [timeline, tMs]
      return
    }
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
      const next = queued.current
      if (next) {
        queued.current = null
        void paint(next[0], next[1])
      }
    }
  }, [canvasRef])

  // The fonts the LIVE session was built with. An edit changes the document,
  // never the fonts — so a doc/data change re-arms in place (keeping the GPU
  // device), and only a font change pays for a fresh session.
  const sessionFonts = useRef<Uint8Array[] | null>(null)

  useEffect(() => {
    let cancelled = false
    // No font gate here: whether fonts are required is the DOCUMENT's fact,
    // and Rust enforces it — a template that names fonts and is given none
    // fails to arm with a sentence naming the families. A boxes-only template
    // arms fontlessly, which is how the designer starts from nothing.
    if (!doc) {
      setStatus('idle')
      return
    }
    setProblem(null)

    ;(async () => {
      try {
        const wasm = await loadWasm()
        if (cancelled) return

        const hashes = Object.keys(images)
        const blobs = hashes.map((h) => images[h]!)

        let s = session.current
        if (s && sessionFonts.current === fonts) {
          // A paint holds a shared borrow of the session across its GPU
          // readback await, and rearm needs the exclusive one — wasm-bindgen
          // enforces Rust's aliasing rules straight across the FFI boundary,
          // and calling in anyway is the "recursive use of an object" panic.
          // So the re-arm politely queues behind the frame in flight.
          for (let i = 0; i < 200 && painting.current; i++) {
            await new Promise((r) => setTimeout(r, 10))
          }
          if (cancelled) return
          // Re-arm in place. Rust arms the edited document FIRST and swaps
          // only on success, so a document that no longer solves leaves the
          // old pixels up with the error as a sentence — never a blank
          // canvas mid-edit.
          s.rearm(JSON.stringify(doc), JSON.stringify(data), hashes, blobs)
        } else {
          setStatus('loading')
          const fresh = await wasm.DesignSession.create(
            JSON.stringify(doc),
            JSON.stringify(data),
            fonts,
            hashes,
            blobs,
          )
          if (cancelled) {
            fresh.free()
            return
          }
          session.current?.free()
          session.current = fresh
          sessionFonts.current = fonts
          s = fresh
        }

        setGeneration((g) => g + 1)
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
  }, [doc, data, fonts, images, canvasRef, paint])

  useEffect(() => () => { session.current?.free(); session.current = null }, [])

  return { status, problem, info, paint, session, generation }
}
