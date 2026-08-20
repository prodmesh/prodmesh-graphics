/**
 * The layer tree — structure on the left, where every design tool keeps it.
 *
 * The tree itself is chrome: it reads the document React already holds, the
 * same way the timeline panel does. What it can never do is EDIT that
 * document — a drop, a rename, a reorder all go through the Rust editors and
 * come back as a new document, so undo covers them and the tree, the canvas,
 * and the playout node can never disagree about what the move meant.
 *
 * # One drop, three edits
 *
 * Dropping a row before or after another reorders; dropping ON a container
 * moves into it; dragging out of a container un-parents. All of them are the
 * single `reparent(id, parent, index)` operation, because to the designer they
 * are one gesture, not three commands.
 *
 * # The keyboard is not a fallback
 *
 * Alt+arrows move the focused layer (up/down within its parent, left to climb
 * out, right to dive into the sibling above). jsdom has no layout, so a
 * simulated drag certifies nothing — the keyboard path is what the tests
 * cover, and it is also simply faster for a designer nudging order.
 *
 * # Names are for the eye
 *
 * Double-click renames. It writes `display_name`; the id underneath never
 * changes, so timeline tracks and field bindings keep addressing the node no
 * matter what it is called this week.
 */

import { useMemo, useRef, useState } from 'react'
import { Clapperboard, Image as ImageIcon, Layers, Shapes, Square, Type } from 'lucide-react'
import type { DesignSession } from './useDesign'

type DocNode = {
  id: string
  display_name?: string
  kind?: string
  children?: DocNode[]
}

type Row = {
  id: string
  label: string
  named: boolean
  kind: string
  depth: number
  parentId: string | null
  /** Position among the parent's children, in the CURRENT document. */
  index: number
  childCount: number
  container: boolean
}

/** Row height in px — the drag math and the CSS must agree on this. */
export const LAYER_ROW_H = 24

/** How far a press travels before it is a drag rather than a click. */
const DRAG_SLOP = 4

const KIND_ICON: Record<string, typeof Square> = {
  box: Square,
  text: Type,
  image: ImageIcon,
  video: Clapperboard,
  shape: Shapes,
  group: Layers,
}

/** Kinds that can contain other layers. */
const CONTAINERS = new Set(['box', 'group'])

function flatten(node: DocNode, depth: number, parentId: string | null, index: number, out: Row[]) {
  const kind = node.kind ?? 'box'
  out.push({
    id: node.id,
    label: node.display_name ?? node.id,
    named: node.display_name != null,
    kind,
    depth,
    parentId,
    index,
    childCount: node.children?.length ?? 0,
    container: CONTAINERS.has(kind),
  })
  node.children?.forEach((c, i) => flatten(c, depth + 1, node.id, i, out))
}

function subtreeIds(node: DocNode, out: Set<string>) {
  out.add(node.id)
  node.children?.forEach((c) => subtreeIds(c, out))
}

function findNode(node: DocNode, id: string): DocNode | null {
  if (node.id === id) return node
  for (const c of node.children ?? []) {
    const hit = findNode(c, id)
    if (hit) return hit
  }
  return null
}

type Drop = { row: number; zone: 'before' | 'after' | 'into'; ok: boolean }

export function LayerTree({
  session,
  doc,
  selected,
  onSelect,
  onEdit,
  onMenu,
}: {
  session: React.RefObject<DesignSession | null>
  doc: Record<string, unknown> | null
  selected: string | null
  onSelect: (id: string | null) => void
  onEdit: (doc: Record<string, unknown>) => void
  /** Right-click on a row: the same element menu the canvas offers. */
  onMenu?: (x: number, y: number, id: string) => void
}) {
  const root = (doc?.root as DocNode | undefined) ?? null
  const rows = useMemo(() => {
    if (!root) return []
    const out: Row[] = []
    flatten(root, 0, null, 0, out)
    return out
  }, [root])

  const listRef = useRef<HTMLDivElement | null>(null)
  const pressed = useRef<{ id: string; x: number; y: number; pointerId: number } | null>(null)
  /** The drag's identity lives in a ref — the move that starts a drag and the
   *  move that positions it can land in one React batch, and the logic must
   *  see the start immediately. The state twin exists only to paint. */
  const dragRef = useRef<string | null>(null)
  const [dragId, setDragId] = useState<string | null>(null)
  const [drop, setDrop] = useState<Drop | null>(null)
  const [renaming, setRenaming] = useState<string | null>(null)
  const [problem, setProblem] = useState<string | null>(null)

  /** Run one session edit; failures are sentences, not silence. */
  const edit = (fn: (s: DesignSession) => string) => {
    const s = session.current
    if (!s) return
    setProblem(null)
    try {
      onEdit(JSON.parse(fn(s)) as Record<string, unknown>)
    } catch (e) {
      setProblem(e instanceof Error ? e.message : String(e))
    }
  }

  /**
   * Where a move lands, in the Rust editor's terms: the index counts
   * positions AFTER the node has left its old place, so a same-parent move to
   * a later slot steps one back.
   */
  const applyMove = (id: string, parent: string, index: number) => {
    const from = rows.find((r) => r.id === id)
    let at = index
    if (from && from.parentId === parent && from.index < index) at -= 1
    edit((s) => s.reparent(id, parent, at))
  }

  const resolveDrop = (d: Drop): { parent: string; index: number } | null => {
    const target = rows[d.row]
    if (!target) return null
    if (d.zone === 'into') return { parent: target.id, index: target.childCount }
    if (!target.parentId) return null // before/after the root means nothing
    return { parent: target.parentId, index: target.index + (d.zone === 'after' ? 1 : 0) }
  }

  const dropAt = (clientY: number): Drop | null => {
    const list = listRef.current
    const dragging = dragRef.current
    if (!list || !dragging || !root) return null
    const r = list.getBoundingClientRect()
    const y = clientY - r.top + list.scrollTop
    const row = Math.max(0, Math.min(rows.length - 1, Math.floor(y / LAYER_ROW_H)))
    const frac = y / LAYER_ROW_H - row
    const target = rows[row]!
    const zone: Drop['zone'] =
      target.container && frac >= 0.3 && frac <= 0.7
        ? 'into'
        : frac < 0.5
          ? 'before'
          : 'after'
    // A subtree cannot land inside itself, and the root row only takes 'into'.
    const dragged = findNode(root, dragging)
    const within = new Set<string>()
    if (dragged) subtreeIds(dragged, within)
    const ok =
      !within.has(target.id) &&
      (zone === 'into' ? target.container : target.parentId != null) &&
      resolveDrop({ row, zone, ok: true }) != null
    return { row, zone, ok }
  }

  const onRowPointerDown = (e: React.PointerEvent, row: Row) => {
    if (e.button !== 0) return
    onSelect(row.id)
    // The root stays put: it IS the canvas.
    if (row.parentId != null) {
      pressed.current = { id: row.id, x: e.clientX, y: e.clientY, pointerId: e.pointerId }
    }
  }

  const onListPointerMove = (e: React.PointerEvent) => {
    const p = pressed.current
    if (!p) return
    if (!dragRef.current) {
      if (Math.hypot(e.clientX - p.x, e.clientY - p.y) < DRAG_SLOP) return
      dragRef.current = p.id
      setDragId(p.id)
      try {
        listRef.current?.setPointerCapture(p.pointerId)
      } catch {
        // Capture is comfort, not correctness: without it a drag that leaves
        // the list just stops updating until the pointer returns.
      }
    }
    setDrop(dropAt(e.clientY))
  }

  const onListPointerUp = (e: React.PointerEvent) => {
    // Recomputed at the release point, not read back from state — the last
    // move and the release can land in one React batch, and the state twin
    // exists only to paint.
    const id = dragRef.current
    const d = id ? dropAt(e.clientY) : null
    pressed.current = null
    dragRef.current = null
    setDragId(null)
    setDrop(null)
    if (!d?.ok || !id) return
    const to = resolveDrop(d)
    if (to) applyMove(id, to.parent, to.index)
  }

  const onListPointerCancel = () => {
    pressed.current = null
    dragRef.current = null
    setDragId(null)
    setDrop(null)
  }

  const onRowKeyDown = (e: React.KeyboardEvent, row: Row) => {
    if (e.key === 'Enter') {
      e.preventDefault()
      setRenaming(row.id)
      return
    }
    if (!e.altKey || row.parentId == null) return
    const siblings = rows.filter((r) => r.parentId === row.parentId)
    if (e.key === 'ArrowUp' && row.index > 0) {
      e.preventDefault()
      applyMove(row.id, row.parentId, row.index - 1)
    } else if (e.key === 'ArrowDown' && row.index < siblings.length - 1) {
      e.preventDefault()
      applyMove(row.id, row.parentId, row.index + 2)
    } else if (e.key === 'ArrowLeft') {
      // Climb out: land right after the parent it just left.
      const parent = rows.find((r) => r.id === row.parentId)
      if (parent?.parentId != null) {
        e.preventDefault()
        applyMove(row.id, parent.parentId, parent.index + 1)
      }
    } else if (e.key === 'ArrowRight') {
      // Dive into the sibling above, when it can hold layers.
      const above = siblings.find((r) => r.index === row.index - 1)
      if (above?.container) {
        e.preventDefault()
        applyMove(row.id, above.id, above.childCount)
      }
    }
  }

  const commitRename = (id: string, value: string) => {
    setRenaming(null)
    const row = rows.find((r) => r.id === id)
    const next = value.trim()
    // Unchanged, or cleared when it was never named: not an edit.
    if (!row || next === row.label || (!next && !row.named)) return
    edit((s) => s.setName(id, next || undefined))
  }

  if (!root) return <p className="design-console__dim">Open a template to see its layers.</p>

  return (
    <div className="lt">
      <div
        ref={listRef}
        className="lt__list"
        role="tree"
        aria-label="Layers"
        onPointerMove={onListPointerMove}
        onPointerUp={onListPointerUp}
        onPointerCancel={onListPointerCancel}
      >
        {rows.map((row, i) => {
          const Icon = KIND_ICON[row.kind] ?? Square
          const dropHere = drop?.row === i ? drop : null
          const cls = [
            'lt__row',
            row.id === selected ? 'lt__row--selected' : '',
            row.id === dragId ? 'lt__row--dragging' : '',
            dropHere ? `lt__row--drop-${dropHere.zone}` : '',
            dropHere && !dropHere.ok ? 'lt__row--drop-bad' : '',
          ]
            .filter(Boolean)
            .join(' ')
          return (
            <div
              key={row.id}
              role="treeitem"
              aria-level={row.depth + 1}
              aria-selected={row.id === selected}
              tabIndex={0}
              className={cls}
              style={{ paddingLeft: 8 + row.depth * 14 }}
              title={row.named ? row.id : undefined}
              onPointerDown={(e) => onRowPointerDown(e, row)}
              onDoubleClick={() => setRenaming(row.id)}
              onKeyDown={(e) => onRowKeyDown(e, row)}
              onContextMenu={(e) => {
                e.preventDefault()
                onSelect(row.id)
                onMenu?.(e.clientX, e.clientY, row.id)
              }}
            >
              <Icon size={12} aria-hidden />
              {renaming === row.id ? (
                <input
                  className="lt__rename"
                  defaultValue={row.named ? row.label : ''}
                  placeholder={row.id}
                  aria-label={`Rename layer ${row.id}`}
                  autoFocus
                  onFocus={(e) => e.target.select()}
                  onBlur={(e) => commitRename(row.id, e.target.value)}
                  onKeyDown={(e) => {
                    e.stopPropagation()
                    if (e.key === 'Enter') (e.target as HTMLInputElement).blur()
                    if (e.key === 'Escape') setRenaming(null)
                  }}
                  onPointerDown={(e) => e.stopPropagation()}
                />
              ) : (
                <span className="lt__label">{row.label}</span>
              )}
              {row.childCount > 0 ? <span className="lt__count">{row.childCount}</span> : null}
            </div>
          )
        })}
      </div>
      {problem ? (
        <p className="tl__problem" role="alert">{problem}</p>
      ) : null}
      <p className="lt__hint">
        Drag to reorder; drop on a box to move inside it. Alt+arrows do the same
        from the keyboard. Double-click renames.
      </p>
    </div>
  )
}
