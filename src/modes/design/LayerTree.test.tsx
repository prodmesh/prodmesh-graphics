/**
 * The layer tree's non-drag paths — which are the certifiable ones: jsdom has
 * no layout, so a simulated drag proves nothing, and the keyboard is the
 * contract the tests can actually hold (the component doc says so too).
 *
 * The session is a stub that speaks the wasm surface's language: it receives
 * the same `reparent`/`setName` calls the real DesignSession would, and
 * returns a document. What those calls DO to a document is Rust's job and is
 * tested in Rust; this file tests that the chrome asks for the right move.
 */

import { describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import { afterEach } from 'vitest'
import { LayerTree } from './LayerTree'
import type { DesignSession } from './useDesign'

afterEach(cleanup)

const doc = {
  root: {
    id: 'root',
    kind: 'box',
    children: [
      { id: 'band', kind: 'box', children: [{ id: 'inner', kind: 'box', children: [] }] },
      { id: 'chip', kind: 'box', display_name: 'Score chip', children: [] },
    ],
  },
} as unknown as Record<string, unknown>

function setup(overrides: Partial<Record<'reparent' | 'setName', unknown>> = {}) {
  const reparent = vi.fn(() => JSON.stringify(doc))
  const setName = vi.fn(() => JSON.stringify(doc))
  const session = {
    current: { reparent, setName, ...overrides } as unknown as DesignSession,
  }
  const onSelect = vi.fn()
  const onEdit = vi.fn()
  render(
    <LayerTree session={session} doc={doc} selected={null} onSelect={onSelect} onEdit={onEdit} />,
  )
  return { reparent, setName, onSelect, onEdit }
}

const row = (label: string) =>
  screen.getAllByRole('treeitem').find((r) => r.textContent?.includes(label))!

describe('LayerTree', () => {
  it('shows the tree with display names over ids, and selects on press', () => {
    const { onSelect } = setup()
    const labels = screen.getAllByRole('treeitem').map((r) => r.textContent)
    expect(labels.some((l) => l?.includes('Score chip'))).toBe(true)
    expect(labels.some((l) => l?.includes('inner'))).toBe(true)

    fireEvent.pointerDown(row('band'), { button: 0 })
    expect(onSelect).toHaveBeenCalledWith('band')
  })

  it('Alt+ArrowDown reorders within the parent, in post-removal terms', () => {
    const { reparent, onEdit } = setup()
    fireEvent.keyDown(row('band'), { key: 'ArrowDown', altKey: true })
    // band is index 0 of root; down means slot 2 pre-removal — 1 after it
    // leaves, which is what the Rust editor expects.
    expect(reparent).toHaveBeenCalledWith('band', 'root', 1)
    expect(onEdit).toHaveBeenCalled()
  })

  it('Alt+ArrowLeft climbs out to land right after the old parent', () => {
    const { reparent } = setup()
    fireEvent.keyDown(row('inner'), { key: 'ArrowLeft', altKey: true })
    expect(reparent).toHaveBeenCalledWith('inner', 'root', 1)
  })

  it('Alt+ArrowRight dives into the sibling above', () => {
    const { reparent } = setup()
    fireEvent.keyDown(row('Score chip'), { key: 'ArrowRight', altKey: true })
    // chip enters band at the end — band already holds one child.
    expect(reparent).toHaveBeenCalledWith('chip', 'band', 1)
  })

  it('the root and the edges refuse to move', () => {
    const { reparent } = setup()
    fireEvent.keyDown(row('root'), { key: 'ArrowDown', altKey: true })
    fireEvent.keyDown(row('band'), { key: 'ArrowUp', altKey: true }) // already first
    fireEvent.keyDown(row('Score chip'), { key: 'ArrowDown', altKey: true }) // already last
    expect(reparent).not.toHaveBeenCalled()
  })

  it('Enter renames through the document, and Escape does not', () => {
    const { setName } = setup()
    fireEvent.keyDown(row('band'), { key: 'Enter' })
    const input = screen.getByLabelText('Rename layer band') as HTMLInputElement
    fireEvent.change(input, { target: { value: 'Lower third' } })
    fireEvent.blur(input)
    expect(setName).toHaveBeenCalledWith('band', 'Lower third')

    fireEvent.keyDown(row('Score chip'), { key: 'Enter' })
    const input2 = screen.getByLabelText('Rename layer chip') as HTMLInputElement
    fireEvent.change(input2, { target: { value: 'ignored' } })
    fireEvent.keyDown(input2, { key: 'Escape' })
    expect(setName).toHaveBeenCalledTimes(1)
  })

  it('a failed edit lands as a sentence, not silence', () => {
    setup({
      reparent: vi.fn(() => {
        throw new Error('b is inside a and cannot contain it')
      }),
    })
    fireEvent.keyDown(row('band'), { key: 'ArrowDown', altKey: true })
    expect(screen.getByRole('alert').textContent).toContain('cannot contain')
  })
})
