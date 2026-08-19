/**
 * The history's contract: edits are undoable, loads are not, and redo dies on
 * a new edit. Small rules, but a designer leans on Cmd+Z hundreds of times an
 * hour and every one of these has a failure mode a person would feel.
 */

import { describe, expect, test } from 'vitest'
import { act, renderHook } from '@testing-library/react'
import { useDocHistory } from './useHistory'

const doc = (n: number) => ({ id: `v${n}` })

describe('the document history', () => {
  test('edits stack and unwind in order', () => {
    const { result } = renderHook(() => useDocHistory())
    act(() => result.current.load(doc(1)))
    act(() => result.current.edit(doc(2)))
    act(() => result.current.edit(doc(3)))

    expect(result.current.doc).toEqual(doc(3))
    expect(result.current.canUndo).toBe(true)

    act(() => result.current.undo())
    expect(result.current.doc).toEqual(doc(2))
    act(() => result.current.undo())
    expect(result.current.doc).toEqual(doc(1))
    // The floor: undoing with nothing behind you is a no-op, not a crash and
    // not a null document.
    expect(result.current.canUndo).toBe(false)
    act(() => result.current.undo())
    expect(result.current.doc).toEqual(doc(1))
  })

  test('redo replays what undo removed, and a new edit kills it', () => {
    const { result } = renderHook(() => useDocHistory())
    act(() => result.current.load(doc(1)))
    act(() => result.current.edit(doc(2)))
    act(() => result.current.undo())
    expect(result.current.canRedo).toBe(true)

    act(() => result.current.redo())
    expect(result.current.doc).toEqual(doc(2))
    expect(result.current.canRedo).toBe(false)

    // Undo, then edit differently: the abandoned future must be GONE.
    // Redoing into it would resurrect a document the designer edited away.
    act(() => result.current.undo())
    act(() => result.current.edit(doc(9)))
    expect(result.current.canRedo).toBe(false)
    expect(result.current.doc).toEqual(doc(9))
  })

  test('a load starts a new timeline — nothing to undo into', () => {
    const { result } = renderHook(() => useDocHistory())
    act(() => result.current.load(doc(1)))
    act(() => result.current.edit(doc(2)))
    act(() => result.current.load(doc(7)))

    // Undoing across a load would silently swap the designer into a document
    // they closed.
    expect(result.current.canUndo).toBe(false)
    expect(result.current.canRedo).toBe(false)
    expect(result.current.doc).toEqual(doc(7))
  })

  test('the stack is bounded and drops the oldest', () => {
    const { result } = renderHook(() => useDocHistory())
    act(() => result.current.load(doc(0)))
    act(() => {
      for (let i = 1; i <= 150; i++) result.current.edit(doc(i))
    })
    act(() => {
      // Unwind everything the history will give back.
      for (let i = 0; i < 200; i++) result.current.undo()
    })
    // 100 snapshots behind the present: the floor is v50, not v0 — the
    // oldest fell away rather than the newest, and nothing crashed.
    expect(result.current.doc).toEqual(doc(50))
    expect(result.current.canUndo).toBe(false)
  })
})
