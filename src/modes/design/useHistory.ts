/**
 * Undo/redo for the template document.
 *
 * # It operates on the SceneDoc, and it lives where the document lives
 *
 * ADR 0019 called this: undo works on the **document**, never on pixels, and
 * belongs wherever the document does. In Design the document is React state,
 * so the history is too — a bounded stack of whole-document snapshots. Whole
 * snapshots rather than inverse operations, deliberately: every edit already
 * arrives as a complete new document from the Rust editors, the documents are
 * kilobytes, and an inverse-op journal is a second implementation of every
 * edit — the class of duplicate this project keeps refusing.
 *
 * # What is history and what is not
 *
 * Document EDITS are history: a drag, a nudge, a style change, an add, a
 * delete. LOADING is not — opening a file or a published template starts a
 * new timeline, because undoing across a load would silently swap the
 * designer into a document they closed. Sample-data changes are not history
 * either: they change the preview, not the template.
 *
 * # Redo dies on a new edit
 *
 * Edit after undo and the future is gone — the universal convention, kept
 * because a branching history needs a UI nobody has ever wanted in a design
 * tool.
 */

import { useCallback, useRef, useState } from 'react'

export type Doc = Record<string, unknown>

/** Snapshots kept behind the present. Beyond this, the oldest fall away. */
const DEPTH = 100

type History = {
  past: Doc[]
  present: Doc | null
  future: Doc[]
}

export function useDocHistory() {
  const [h, setH] = useState<History>({ past: [], present: null, future: [] })
  // Monotonic count of *timeline replacements* (loads), so consumers can
  // reset per-document state (selection) on a load but not on an edit.
  const loads = useRef(0)

  /** Open a document: a new timeline. Nothing to undo into. */
  const load = useCallback((doc: Doc | null) => {
    loads.current += 1
    setH({ past: [], present: doc, future: [] })
  }, [])

  /** Commit an edit: the present becomes undoable, the future is gone. */
  const edit = useCallback((doc: Doc) => {
    setH((cur) => ({
      past: cur.present ? [...cur.past.slice(-(DEPTH - 1)), cur.present] : cur.past,
      present: doc,
      future: [],
    }))
  }, [])

  const undo = useCallback(() => {
    setH((cur) => {
      const prev = cur.past[cur.past.length - 1]
      if (!prev || !cur.present) return cur
      return {
        past: cur.past.slice(0, -1),
        present: prev,
        future: [cur.present, ...cur.future],
      }
    })
  }, [])

  const redo = useCallback(() => {
    setH((cur) => {
      const next = cur.future[0]
      if (!next || !cur.present) return cur
      return {
        past: [...cur.past, cur.present],
        present: next,
        future: cur.future.slice(1),
      }
    })
  }, [])

  return {
    doc: h.present,
    load,
    edit,
    undo,
    redo,
    canUndo: h.past.length > 0,
    canRedo: h.future.length > 0,
    /** Bumps on load only — key per-document state on this. */
    loadCount: loads.current,
  }
}
