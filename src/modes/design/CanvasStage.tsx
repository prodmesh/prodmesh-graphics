/**
 * The canvas with its interaction overlay — the React half of ADR 0019.
 *
 * # Who owns what
 *
 * Rust owns the geometry: what is under the pointer, where the handles sit,
 * what a drag proposes, what a finished drag writes into the document. This
 * component owns the GESTURE — pointer capture, keyboard focus, and drawing
 * the chrome at the coordinates Rust reports. It computes nothing spatial
 * beyond scaling client pixels to canvas pixels, once, at the edge.
 *
 * # The overlay is SVG with the canvas's own viewBox
 *
 * Every coordinate Rust returns is a design-space pixel, and the overlay's
 * viewBox IS the design space — so handles, ghosts and guides are drawn with
 * the numbers as they arrive, and CSS scaling of the canvas scales the chrome
 * with it for free. Handle size is the one exception: a grab target must stay
 * ~finger-sized on SCREEN, so it is divided by the current CSS scale.
 *
 * # A drag ghosts, then commits
 *
 * During a drag the document is untouched; the proposal renders as an outline
 * over the old pixels. On release the edit lands in the document and the whole
 * template re-arms — the same solve the node will run. The pixels the designer
 * ends up looking at are therefore never an editor's approximation.
 */

import { useCallback, useEffect, useRef, useState } from 'react'
import type { DesignSession, DragResult, Handles } from './useDesign'

/** Selection chrome colour duty: cyan is ARMED — the operator's intent — and
 *  a selection is exactly that. Red stays reserved for ON AIR (ADR 0020). */

const HANDLE_SCREEN_PX = 8

type HandleName = 'nw' | 'n' | 'ne' | 'e' | 'se' | 's' | 'sw' | 'w'

const CURSORS: Record<HandleName, string> = {
  nw: 'nwse-resize',
  se: 'nwse-resize',
  ne: 'nesw-resize',
  sw: 'nesw-resize',
  n: 'ns-resize',
  s: 'ns-resize',
  e: 'ew-resize',
  w: 'ew-resize',
}

const mid = (a: [number, number], b: [number, number]): [number, number] => [
  (a[0] + b[0]) / 2,
  (a[1] + b[1]) / 2,
]

export function CanvasStage({
  canvasRef,
  session,
  ready,
  generation,
  selected,
  onSelect,
  onEdit,
  onDelete,
  onMenu,
}: {
  canvasRef: React.RefObject<HTMLCanvasElement | null>
  session: React.RefObject<DesignSession | null>
  ready: boolean
  /**
   * The session's replacement counter, NOT the document: the ref swap is
   * invisible to React, and an effect keyed on the document re-runs before
   * the new session exists — stale geometry that never refreshes when the
   * loading→ready flip lands in one commit (fast adapter). Geometry is only
   * new when the session is.
   */
  generation: number
  selected: string | null
  onSelect: (id: string | null) => void
  /** A finished edit: the new document, parsed. Re-arms upstream. */
  onEdit: (doc: Record<string, unknown>) => void
  /** Delete the selection. The owner decides what is deletable (never the
   *  root), so this fires unconditionally on the key. */
  onDelete?: () => void
  /** Right-click: the hit node (already selected) or null for empty canvas,
   *  at viewport coordinates for a menu. */
  onMenu?: (x: number, y: number, id: string | null) => void
}) {
  const wrapRef = useRef<HTMLDivElement | null>(null)
  const svgRef = useRef<SVGSVGElement | null>(null)
  const [handles, setHandles] = useState<Handles | null>(null)
  const [ghost, setGhost] = useState<DragResult | null>(null)
  const [dragging, setDragging] = useState(false)
  // Canvas px per screen px, so handles keep their screen size.
  const [scale, setScale] = useState(1)

  const dims = () => {
    const s = session.current
    return s ? { w: s.width, h: s.height } : null
  }

  // Track the CSS scale. The canvas is max-width constrained, so its client
  // width changes with the panel; handles must not shrink with it.
  useEffect(() => {
    const el = wrapRef.current
    if (!el) return
    const update = () => {
      const d = dims()
      if (d && el.clientWidth > 0) setScale(d.w / el.clientWidth)
    }
    update()
    // jsdom has neither layout nor ResizeObserver; scale stays 1 there, which
    // is also the truth — nothing is scaled in a test.
    if (typeof ResizeObserver === 'undefined') return
    const ro = new ResizeObserver(update)
    ro.observe(el)
    return () => ro.disconnect()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [ready, generation])

  // Refresh the selection chrome whenever the geometry may have moved: a new
  // document, a re-arm, a different selection. A node that no longer exists
  // (deleted, or collapsed by visible_if) clears the selection rather than
  // drawing chrome around nothing.
  // A handles miss remembered as (generation, id), so a missing node is only
  // deselected once a NEWER session has confirmed it is really gone.
  const missed = useRef<{ gen: number; id: string } | null>(null)

  useEffect(() => {
    const s = session.current
    if (!ready || !s || !selected) {
      setHandles(null)
      return
    }
    try {
      setHandles(JSON.parse(s.handles(selected)) as Handles)
      missed.current = null
    } catch {
      // The session does not know this node — which happens for one commit
      // whenever the selection is a node that was JUST added: the effect runs
      // against the old session before the re-arm lands. Clearing the
      // selection here threw away every freshly-added node's selection, so a
      // first miss only clears the chrome. A second miss from a NEWER session
      // means the node is genuinely gone (deleted, or collapsed by
      // visible_if), and then the selection goes too.
      setHandles(null)
      if (missed.current && missed.current.id === selected && missed.current.gen < generation) {
        missed.current = null
        onSelect(null)
      } else {
        missed.current = { gen: generation, id: selected }
      }
    }
  }, [ready, generation, selected, session, onSelect])

  const toCanvas = useCallback(
    (e: { clientX: number; clientY: number }): [number, number] | null => {
      const svg = svgRef.current
      const d = dims()
      if (!svg || !d) return null
      const r = svg.getBoundingClientRect()
      if (r.width === 0 || r.height === 0) return null
      return [((e.clientX - r.left) * d.w) / r.width, ((e.clientY - r.top) * d.h) / r.height]
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [],
  )

  const onPointerDown = useCallback(
    (e: React.PointerEvent) => {
      // Primary button only. A right-click used to begin a drag whose
      // pointerup went to the browser's context menu instead — a phantom drag
      // that edited the template on the next stray move.
      if (e.button !== 0) return
      const s = session.current
      const p = toCanvas(e)
      if (!s || !p) return
      wrapRef.current?.focus()

      const handle = (e.target as Element).getAttribute?.('data-handle') as
        | HandleName
        | 'move'
        | null

      if (handle && selected) {
        s.beginDrag(selected, handle, p[0], p[1])
      } else {
        const hit = s.hitTest(p[0], p[1]) ?? null
        onSelect(hit)
        if (!hit) return
        s.beginDrag(hit, 'move', p[0], p[1])
      }
      setDragging(true)
      ;(e.currentTarget as Element).setPointerCapture(e.pointerId)
      e.preventDefault()
    },
    [selected, onSelect, session, toCanvas],
  )

  const onPointerMove = useCallback(
    (e: React.PointerEvent) => {
      const s = session.current
      const p = toCanvas(e)
      if (!dragging || !s || !p) return
      try {
        // Alt disables snapping — the convention every design tool shares.
        setGhost(JSON.parse(s.dragTo(p[0], p[1], !e.altKey)) as DragResult)
      } catch {
        setGhost(null)
      }
    },
    [dragging, session, toCanvas],
  )

  const onPointerUp = useCallback(
    (e: React.PointerEvent) => {
      const s = session.current
      const p = toCanvas(e)
      setDragging(false)
      setGhost(null)
      if (!s || !p) return
      try {
        const edited = s.endDrag(p[0], p[1], !e.altKey)
        // null means the drag never moved: a click, not an edit.
        if (edited) onEdit(JSON.parse(edited) as Record<string, unknown>)
      } catch {
        // The document did not change; the canvas still shows the truth.
      }
    },
    [session, toCanvas, onEdit],
  )

  const onKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      const s = session.current
      if (!s) return
      if (e.key === 'Escape') {
        if (dragging) {
          s.cancelDrag()
          setDragging(false)
          setGhost(null)
        } else {
          onSelect(null)
        }
        return
      }
      if (!selected) return
      if (e.key === 'Delete' || e.key === 'Backspace') {
        e.preventDefault()
        onDelete?.()
        return
      }
      const step = e.shiftKey ? 10 : 1
      const d: Record<string, [number, number]> = {
        ArrowLeft: [-step, 0],
        ArrowRight: [step, 0],
        ArrowUp: [0, -step],
        ArrowDown: [0, step],
      }
      const delta = d[e.key]
      if (!delta) return
      e.preventDefault()
      try {
        onEdit(JSON.parse(s.nudge(selected, delta[0], delta[1])) as Record<string, unknown>)
      } catch {
        // The node may be gone; the selection effect will clear it.
      }
    },
    [selected, dragging, session, onSelect, onEdit, onDelete],
  )

  const d = ready ? dims() : null
  const hs = HANDLE_SCREEN_PX * scale
  const c = handles?.corners

  return (
    <div
      ref={wrapRef}
      className="dsg-stage"
      tabIndex={0}
      role="application"
      aria-label="Template canvas"
      onKeyDown={onKeyDown}
    >
      <div className="design__checker">
        <canvas ref={canvasRef} className="design__canvas" />
        {d ? (
          <svg
            ref={svgRef}
            className="dsg-overlay"
            viewBox={`0 0 ${d.w} ${d.h}`}
            onPointerDown={onPointerDown}
            onPointerMove={onPointerMove}
            onPointerUp={onPointerUp}
            onContextMenu={(e) => {
              // Ours, not the browser's. Right-click selects what it hit —
              // the convention every design tool shares — and asks the owner
              // for a menu there.
              e.preventDefault()
              const s = session.current
              const p = toCanvas(e)
              if (!s || !p) return
              const hit = s.hitTest(p[0], p[1]) ?? null
              if (hit) onSelect(hit)
              onMenu?.(e.clientX, e.clientY, hit)
            }}
          >
            {handles && c ? (
              <g>
                <polygon
                  className="dsg-outline"
                  points={c.map((p) => p.join(',')).join(' ')}
                  data-handle="move"
                  style={{ strokeWidth: scale }}
                />
                {(
                  [
                    ['nw', c[0]],
                    ['n', mid(c[0], c[1])],
                    ['ne', c[1]],
                    ['e', mid(c[1], c[2])],
                    ['se', c[2]],
                    ['s', mid(c[2], c[3])],
                    ['sw', c[3]],
                    ['w', mid(c[3], c[0])],
                  ] as [HandleName, [number, number]][]
                ).map(([name, p]) => (
                  <rect
                    key={name}
                    className="dsg-handle"
                    data-handle={name}
                    x={p[0] - hs / 2}
                    y={p[1] - hs / 2}
                    width={hs}
                    height={hs}
                    style={{ cursor: CURSORS[name], strokeWidth: scale }}
                  />
                ))}
              </g>
            ) : null}

            {ghost ? (
              <g>
                {ghost.guides.map((g, i) =>
                  g.axis === 0 ? (
                    <line key={i} className="dsg-guide" x1={g.at} y1={0} x2={g.at} y2={d.h}
                          style={{ strokeWidth: scale }} />
                  ) : (
                    <line key={i} className="dsg-guide" x1={0} y1={g.at} x2={d.w} y2={g.at}
                          style={{ strokeWidth: scale }} />
                  ),
                )}
                <rect
                  className="dsg-ghost"
                  x={ghost.rect[0]}
                  y={ghost.rect[1]}
                  width={ghost.rect[2]}
                  height={ghost.rect[3]}
                  style={{ strokeWidth: scale, strokeDasharray: `${4 * scale} ${3 * scale}` }}
                />
              </g>
            ) : null}
          </svg>
        ) : null}
      </div>
    </div>
  )
}
