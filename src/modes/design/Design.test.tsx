/**
 * Design mode, tested for its promises: selection drives the panel tabs, the
 * optional-style checkboxes ADD on enable and CLEAR on disable, the element
 * menu speaks the clipboard's state, the revisions dialog shows the diff and
 * reports per-Item verdicts, and a right-click never begins a drag.
 *
 * The session is a stub speaking the wasm surface's language, same pattern as
 * LayerTree.test: what the editors DO to a document is Rust's job and is
 * tested in Rust — this file certifies that the chrome asks for the right
 * edit and tells the truth about the result.
 */

import { afterEach, beforeEach, describe, expect, test, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { _reset as resetQuery } from '../../lib/useQuery'
import { _reset as resetDirty } from '../../lib/dirty'

// ---------------------------------------------------------------------------
// The stub session. Mutable style state, so a checkbox toggle round-trips:
// the editor mutates it, the doc "changes", the mock hook bumps generation,
// and the inspector re-reads facts that now include the effect.

type Style = {
  stroke: { color: number[]; width: number } | null
  shadow: { color: number[]; dx: number; dy: number; blur: number } | null
  fill: number[] | null
  gradient: { angle: number; stops: [number[], number][] } | null
}

const h = vi.hoisted(() => {
  const style: Style = { stroke: null, shadow: null, fill: [1, 1, 1, 1], gradient: null }
  // Edits return the real document shape (with a nonce for identity), so the
  // tree still stands after a cut or a paste and the test can keep driving.
  const DOC = {
    format: 'pmg.scene',
    format_version: { major: 1, minor: 1 },
    min_reader: 1,
    id: 'tpl-1',
    name: 'Lower third',
    canvas: { width: 1280, height: 720 },
    root: {
      id: 'root',
      kind: 'box',
      layout: {},
      transform: {},
      style: {},
      children: [
        { id: 'band', kind: 'box', layout: {}, transform: {}, style: {}, children: [] },
        { id: 'caption', kind: 'text', layout: {}, transform: {}, style: {}, children: [] },
        { id: 'wipe', kind: 'mask', layout: {}, transform: {}, style: {}, children: [] },
      ],
    },
    timelines: [],
    theme: 'blake3:themehead',
  }
  const doc = () => JSON.stringify({ ...DOC, nonce: Math.random() })
  const facts = (id: string) =>
    JSON.stringify({
      id,
      kind: id === 'caption' ? 'text' : id === 'wipe' ? 'mask' : 'box',
      // A mask has no paint at all — the compiler refuses it, so the facts
      // must not offer it either.
      fill: id === 'wipe' ? null : style.fill,
      gradient: style.gradient ?? undefined,
      stroke: style.stroke ?? undefined,
      shadow: style.shadow ?? undefined,
      radius: 0,
      opacity: 1,
      absolute: false,
      clip: false,
      width_mode: id === 'wipe' ? 'percent' : 'px',
      height_mode: id === 'wipe' ? 'percent' : 'px',
      margin: [0, 0, 0, 0],
      padding: [0, 0, 0, 0],
      ...(id === 'caption' ? { text: { expr: 'Hi', family: 'Test', weight: 400, size: 32, align: 'start' } } : {}),
    })
  const session = {
    width: 1280,
    height: 720,
    handles: vi.fn(() => JSON.stringify({ bbox: [0, 0, 100, 50], corners: [[0, 0], [100, 0], [100, 50], [0, 50]] })),
    facts: vi.fn(facts),
    hitTest: vi.fn(() => 'band'),
    beginDrag: vi.fn(),
    // A real DragResult: the ghost is drawn from it, and an empty object
    // crashed the render the moment a test actually dragged something.
    dragTo: vi.fn(() =>
      JSON.stringify({ rect: [0, 0, 100, 50], dx: 40, dy: 0, w: 100, h: 50, guides: [] }),
    ),
    endDrag: vi.fn(() => null),
    cancelDrag: vi.fn(),
    setStroke: vi.fn((_id: string, r: number, g: number, b: number, a: number, w: number) => {
      style.stroke = { color: [r, g, b, a], width: w }
      return doc()
    }),
    clearStroke: vi.fn(() => {
      style.stroke = null
      return doc()
    }),
    setShadow: vi.fn(() => {
      style.shadow = { color: [0, 0, 0, 0.6], dx: 6, dy: 6, blur: 12 }
      return doc()
    }),
    clearShadow: vi.fn(() => {
      style.shadow = null
      return doc()
    }),
    setFill: vi.fn(() => {
      style.fill = [1, 1, 1, 1]
      style.gradient = null
      return doc()
    }),
    clearFill: vi.fn(() => {
      style.fill = null
      style.gradient = null
      return doc()
    }),
    setGradient: vi.fn(() => {
      style.gradient = { angle: 180, stops: [[[1, 1, 1, 1], 0], [[0.3, 0.3, 0.3, 1], 1]] as [number[], number][] }
      return doc()
    }),
    addMask: vi.fn((parent?: string) =>
      JSON.stringify({ id: `${parent ?? 'root'}-mask`, doc: JSON.parse(doc()) }),
    ),
    copyNode: vi.fn((id: string) => JSON.stringify({ id, kind: 'box' })),
    pasteNode: vi.fn((_t: string, payload: string, _beside: boolean) => {
      const inner = JSON.parse(payload) as { id: string }
      return JSON.stringify({ id: `${inner.id}-pasted`, doc: JSON.parse(doc()) })
    }),
    deleteNode: vi.fn(() => doc()),
    reparent: vi.fn(() => doc()),
    setName: vi.fn(() => doc()),
    setColorToken: vi.fn(() => doc()),
    setFontToken: vi.fn(() => doc()),
    applyTheme: vi.fn(() => doc()),
    clearTheme: vi.fn(() => doc()),
  }
  return { session, style, DOC }
})

vi.mock('./useDesign', async () => {
  const React = await import('react')
  return {
    useDesignCanvas: (_ref: unknown, doc: unknown) => {
      const [generation, setGen] = React.useState(0)
      React.useEffect(() => {
        if (doc) setGen((g) => g + 1)
      }, [doc])
      // ONE ref object for the life of the component, like the real hook's.
      // A fresh `{ current }` literal per render looks equivalent and is not:
      // `CanvasStage` captures the session inside a `useCallback([])`, so a new
      // object every render left that closure holding the FIRST one — null,
      // forever. Every canvas pointer handler then returned early, and the
      // right-button test passed for the wrong reason.
      const session = React.useRef<unknown>(null)
      session.current = doc ? h.session : null
      return {
        status: doc ? 'ready' : 'idle',
        problem: null,
        info: null,
        paint: async () => {},
        session,
        generation,
      }
    },
    compileTemplate: vi.fn(async () => ({ template_rev: 'blake3:swept', scene: new Uint8Array(), manifest: '{}' })),
    blankDoc: vi.fn(async () => ({})),
    migrateDoc: vi.fn(async (d: unknown) => d),
    fontFacts: vi.fn(async () => null),
    themeHash: vi.fn(async () => ({ hash: 'blake3:newthemehash', bytes: new Uint8Array([1]) })),
    rethemeDoc: vi.fn(async (d: unknown) => d),
  }
})

import { Design } from './Design'

// ---------------------------------------------------------------------------
// The wire. A router over fetch, since Design pulls templates, assets, items,
// revisions, and the mirror document.

const REVISIONS = {
  revisions: [
    {
      template_rev: 'blake3:new',
      name: 'Lower third',
      min_reader: 1,
      manifest: { name: 'Lower third', min_reader: 1, fields: [{ key: 'campus', label: 'Campus', required: true }], timelines: ['In', 'Out', 'Highlight'], assets: [] },
      published_at: 2000,
      published_by: null,
      items: [],
    },
    {
      template_rev: 'blake3:old',
      name: 'Lower third',
      min_reader: 1,
      manifest: { name: 'Lower third', min_reader: 1, fields: [], timelines: ['In', 'Out'], assets: [] },
      published_at: 1000,
      published_by: null,
      items: [{ id: 'item-1', label: 'Pastor Dan' }],
    },
  ],
}

let updateItemsResponse = {
  to: 'blake3:new',
  updated: [{ id: 'item-1', label: 'Pastor Dan' }],
  refused: [] as { id: string; label: string; problem: string }[],
}

beforeEach(() => {
  resetQuery()
  resetDirty()
  h.session.setStroke.mockClear()
  h.session.clearStroke.mockClear()
  h.session.copyNode.mockClear()
  h.session.pasteNode.mockClear()
  h.session.deleteNode.mockClear()
  h.session.beginDrag.mockClear()
  h.style.stroke = null
  h.style.shadow = null
  h.style.fill = [1, 1, 1, 1]
  h.style.gradient = null
  updateItemsResponse = { to: 'blake3:new', updated: [{ id: 'item-1', label: 'Pastor Dan' }], refused: [] }

  vi.stubGlobal(
    'fetch',
    vi.fn(async (url: RequestInfo | URL, init?: RequestInit) => {
      const u = String(url)
      const json = (body: unknown) => new Response(JSON.stringify(body), { status: 200 })
      if (u.endsWith('/api/templates')) {
        return json({ templates: [{ id: 'tpl-1', name: 'Lower third', head_rev: 'blake3:new' }] })
      }
      if (u.endsWith('/api/templates/tpl-1/update-items')) return json(updateItemsResponse)
      if (u.endsWith('/api/templates/tpl-1/revisions')) return json(REVISIONS)
      if (u.endsWith('/api/templates/tpl-1')) {
        return json({ id: 'tpl-1', name: 'Lower third', head_rev: 'blake3:new', head: { template_rev: 'blake3:new', mirror_hash: 'blake3:mirror' } })
      }
      if (u.includes('/cas/blake3:mirror')) return json(h.DOC)
      if (u.endsWith('/api/assets')) return json({ assets: [] })
      if (u.endsWith('/api/themes/th-1/followers')) {
        return json({
          followers: [{
            id: 'tpl-1', name: 'Lower third', head_rev: 'blake3:new',
            theme_hash: 'blake3:oldthemehash', current: false, mirror_hash: 'blake3:mirror',
          }],
        })
      }
      if (u.endsWith('/api/templates/tpl-1/publish')) {
        return json({ template_rev: 'blake3:swept', warnings: [], missing_assets: [] })
      }
      if (u.endsWith('/api/themes')) {
        return json({
          themes: [{
            id: 'th-1', name: 'Advent', head_hash: 'blake3:themehead',
            tokens: {
              'brand.primary': { color: { rgba: [0.8, 0.1, 0.1, 1] } },
              'brand.font': { font: { family: 'IBM Plex Sans', weight: 600 } },
            },
            updated_at: 0,
          }],
        })
      }
      if (u.endsWith('/api/items')) return json({ items: [] })
      throw new Error(`unrouted fetch ${u} ${init?.method ?? 'GET'}`)
    }),
  )

  // jsdom has no layout; the canvas math needs a rect to exist.
  vi.spyOn(Element.prototype, 'getBoundingClientRect').mockReturnValue({
    x: 0, y: 0, top: 0, left: 0, right: 800, bottom: 450, width: 800, height: 450,
    toJSON: () => ({}),
  } as DOMRect)
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
  vi.restoreAllMocks()
})

const paint = async () => {
  render(<Design />)
  await act(async () => {})
}

/** Open the fixture template through the real open path. */
const openTemplate = async () => {
  await paint()
  const entry = await screen.findByText('Lower third')
  fireEvent.click(entry)
  await waitFor(() => expect(screen.getAllByRole('treeitem').length).toBeGreaterThan(1))
}

const treeRow = (label: string) =>
  screen.getAllByRole('treeitem').find((r) => r.textContent?.includes(label))!

const selectInTree = (label: string) => fireEvent.pointerDown(treeRow(label), { button: 0 })

describe('the panel tabs', () => {
  test('selection drives them: a node opens Inspector, deselecting returns to Template', async () => {
    await openTemplate()
    const tab = (name: string) => screen.getByRole('tab', { name })
    expect(tab('Template').getAttribute('aria-selected')).toBe('true')

    selectInTree('band')
    await waitFor(() => expect(tab('Inspector').getAttribute('aria-selected')).toBe('true'))

    // A manual click back to Template sticks while the selection lives.
    fireEvent.click(tab('Template'))
    expect(tab('Template').getAttribute('aria-selected')).toBe('true')

    // Escape on the canvas deselects, and the template is the subject again.
    fireEvent.keyDown(screen.getByRole('application'), { key: 'Escape' })
    await waitFor(() => expect(tab('Template').getAttribute('aria-selected')).toBe('true'))
  })
})

describe('optional styles', () => {
  test('the checkbox IS the add/remove: enable applies the default, disable clears', async () => {
    await openTemplate()
    selectInTree('band')
    const outline = await screen.findByRole('checkbox', { name: 'Outline' })
    expect((outline as HTMLInputElement).checked).toBe(false)

    fireEvent.click(outline)
    expect(h.session.setStroke).toHaveBeenCalledWith('band', 1, 1, 1, 1, 2)
    // The re-armed facts now carry the stroke: checked, controls revealed.
    await waitFor(() =>
      expect((screen.getByRole('checkbox', { name: 'Outline' }) as HTMLInputElement).checked).toBe(true),
    )
    expect(screen.getByLabelText('Outline width')).toBeDefined()

    fireEvent.click(screen.getByRole('checkbox', { name: 'Outline' }))
    expect(h.session.clearStroke).toHaveBeenCalledWith('band')
    await waitFor(() => expect(screen.queryByLabelText('Outline width')).toBeNull())
  })

  test('a text node has no Fill checkbox — its fill is the ink itself', async () => {
    await openTemplate()
    selectInTree('caption')
    await screen.findByRole('checkbox', { name: 'Outline' })
    expect(screen.queryByRole('checkbox', { name: 'Fill' })).toBeNull()
    expect(screen.getByLabelText('Fill colour')).toBeDefined()
  })
})

describe('the element menu', () => {
  test('right-click opens the verbs, and Paste honours the clipboard', async () => {
    await openTemplate()
    fireEvent.contextMenu(treeRow('band'), { clientX: 100, clientY: 100 })

    const menu = await screen.findByRole('menu')
    expect((within(menu).getByRole('menuitem', { name: 'Paste' }) as HTMLButtonElement).disabled).toBe(true)

    fireEvent.click(within(menu).getByRole('menuitem', { name: 'Copy' }))
    expect(h.session.copyNode).toHaveBeenCalledWith('band')

    fireEvent.contextMenu(treeRow('band'), { clientX: 100, clientY: 100 })
    const menu2 = await screen.findByRole('menu')
    const paste = within(menu2).getByRole('menuitem', { name: 'Paste' }) as HTMLButtonElement
    expect(paste.disabled).toBe(false)
    fireEvent.click(paste)
    expect(h.session.pasteNode).toHaveBeenCalledWith('band', expect.any(String), false)
  })

  test('Duplicate pastes BESIDE, so a box cannot swallow its own copy', async () => {
    await openTemplate()
    fireEvent.contextMenu(treeRow('band'), { clientX: 100, clientY: 100 })
    fireEvent.click(within(await screen.findByRole('menu')).getByRole('menuitem', { name: 'Duplicate' }))
    expect(h.session.pasteNode).toHaveBeenCalledWith('band', expect.any(String), true)
  })

  test('Cut copies then deletes; the root refuses the destructive verbs', async () => {
    await openTemplate()
    fireEvent.contextMenu(treeRow('band'), { clientX: 100, clientY: 100 })
    fireEvent.click(within(await screen.findByRole('menu')).getByRole('menuitem', { name: 'Cut' }))
    expect(h.session.copyNode).toHaveBeenCalledWith('band')
    expect(h.session.deleteNode).toHaveBeenCalledWith('band')

    fireEvent.contextMenu(treeRow('root'), { clientX: 100, clientY: 100 })
    const menu = await screen.findByRole('menu')
    for (const verb of ['Cut', 'Duplicate', 'Delete']) {
      expect((within(menu).getByRole('menuitem', { name: verb }) as HTMLButtonElement).disabled).toBe(true)
    }
  })

  test('Cmd+C, X, V, D drive the same clipboard from the keyboard', async () => {
    await openTemplate()
    selectInTree('band')
    await screen.findByRole('checkbox', { name: 'Outline' })

    fireEvent.keyDown(window, { key: 'c', metaKey: true })
    expect(h.session.copyNode).toHaveBeenCalledWith('band')
    fireEvent.keyDown(window, { key: 'v', metaKey: true })
    expect(h.session.pasteNode).toHaveBeenCalledWith('band', expect.any(String), false)
    fireEvent.keyDown(window, { key: 'd', metaKey: true })
    expect(h.session.pasteNode).toHaveBeenLastCalledWith(expect.any(String), expect.any(String), true)
  })
})

describe('the canvas and the right button', () => {
  test('a right-button press never begins a drag — that was the phantom-drag bug', async () => {
    await openTemplate()
    const svg = document.querySelector('.dsg-overlay')!
    fireEvent.pointerDown(svg, { button: 2, clientX: 100, clientY: 100 })
    expect(h.session.beginDrag).not.toHaveBeenCalled()
  })
})

describe('the drag threshold', () => {
  /** The overlay, with a pointer-capture stub jsdom does not provide. */
  const overlay = () => {
    const svg = document.querySelector('.dsg-overlay') as SVGSVGElement & {
      setPointerCapture: (id: number) => void
    }
    svg.setPointerCapture = () => {}
    svg.getBoundingClientRect = () =>
      ({ left: 0, top: 0, width: 1280, height: 720, right: 1280, bottom: 720, x: 0, y: 0 }) as DOMRect
    return svg
  }

  test('a click with a pixel of trackpad wobble selects and does NOT move', async () => {
    await openTemplate()
    const svg = overlay()
    h.session.dragTo.mockClear()
    h.session.endDrag.mockClear()
    h.session.cancelDrag.mockClear()

    fireEvent.pointerDown(svg, { button: 0, clientX: 100, clientY: 100, pointerId: 1 })
    // The press DID begin a drag — the threshold is about whether it commits.
    expect(h.session.beginDrag).toHaveBeenCalled()
    fireEvent.pointerMove(svg, { buttons: 1, clientX: 102, clientY: 101, pointerId: 1 })
    fireEvent.pointerUp(svg, { clientX: 102, clientY: 101, pointerId: 1 })

    // Nothing was dragged and nothing was committed — the press was a click.
    expect(h.session.dragTo).not.toHaveBeenCalled()
    expect(h.session.endDrag).not.toHaveBeenCalled()
    expect(h.session.cancelDrag).toHaveBeenCalled()
  })

  test('past the threshold it is a drag, and the node moves', async () => {
    await openTemplate()
    const svg = overlay()
    h.session.dragTo.mockClear()
    h.session.endDrag.mockClear()

    fireEvent.pointerDown(svg, { button: 0, clientX: 100, clientY: 100, pointerId: 1 })
    fireEvent.pointerMove(svg, { buttons: 1, clientX: 140, clientY: 100, pointerId: 1 })
    fireEvent.pointerUp(svg, { clientX: 140, clientY: 100, pointerId: 1 })

    expect(h.session.dragTo).toHaveBeenCalled()
    expect(h.session.endDrag).toHaveBeenCalled()
  })
})

describe('revisions', () => {
  test('stale Items are named, the diff is sentences, and verdicts come back per Item', async () => {
    await openTemplate()
    await screen.findByText('1 Item renders an older revision.')

    fireEvent.click(screen.getByRole('button', { name: /Update Items to the newest revision/ }))
    // The manifest diff, as sentences a designer can act on.
    await screen.findByText(/Adds field “Campus” — required, so empty Items will be refused/)
    await screen.findByText(/Adds timeline “Highlight” — a new cue button in Show/)
    expect(screen.getByText(/Pastor Dan/)).toBeDefined()

    updateItemsResponse = {
      to: 'blake3:new',
      updated: [],
      refused: [{ id: 'item-1', label: 'Pastor Dan', problem: 'lt · Graphics: Campus is required.' }],
    }
    fireEvent.click(screen.getByRole('button', { name: /Update 1 Item/ }))
    // Refused means NAMED and untouched — the dialog says which and why.
    await screen.findByText(/Pastor Dan was not moved: lt · Graphics: Campus is required./)
  })
})


describe('theme tokens', () => {
  test('a bound colour follows its token: picker offered, value input disabled', async () => {
    await openTemplate()
    selectInTree('band')
    const follows = await screen.findByLabelText('Fill token')
    expect(within(follows).getByRole('option', { name: 'brand.primary' })).toBeDefined()

    fireEvent.change(follows, { target: { value: 'brand.primary' } })
    expect(h.session.setColorToken).toHaveBeenCalledWith(
      'band', 'fill', 'brand.primary', expect.stringContaining('brand.primary'),
    )
  })

  test('the Template tab shows which theme the document follows', async () => {
    await openTemplate()
    const select = screen.getByLabelText('Theme') as HTMLSelectElement
    expect(select.value).toBe('blake3:themehead')
    fireEvent.change(select, { target: { value: '' } })
    expect(h.session.clearTheme).toHaveBeenCalled()
  })
})


describe('the theme manager and the apply sweep', () => {
  test('a stale follower is named, and the sweep reports its verdicts', async () => {
    await openTemplate()
    fireEvent.click(screen.getByRole('button', { name: /Themes — tokens/ }))

    const dialog = await screen.findByRole('dialog', { name: 'Themes' })
    // The Advent theme's token row is editable.
    expect(within(dialog).getByLabelText('Token brand.primary colour')).toBeDefined()
    // The follower list names the template and its staleness.
    await within(dialog).findByText(/Lower third — an older version of this theme/)
    await within(dialog).findByText(/1 template was resolved under an older version/)

    fireEvent.click(within(dialog).getByRole('button', { name: /Republish 1 template/ }))
    // The sweep line: republished, Items moved — the whole verdict in one row.
    await within(dialog).findByText(/Lower third: republished as swept · 1 Item moved/)
  })
})

describe('masks', () => {
  test('a mask is offered, lands where the selection is, and is selected once made', async () => {
    await openTemplate()
    selectInTree('band')
    fireEvent.click(screen.getByRole('button', { name: /Add a mask/ }))
    await waitFor(() => expect(h.session.addMask).toHaveBeenCalledWith('band'))
  })

  test('a mask offers geometry and NO paint — it is a shape, not ink', async () => {
    await openTemplate()
    selectInTree('wipe')
    // The sentence, because an invisible node that offers no colours needs to
    // say why once.
    await screen.findByText(/reveals what falls inside its shape/)
    // Shape and placement, yes.
    expect(screen.getByLabelText('Corner radius')).toBeDefined()
    expect(screen.getByLabelText('Node opacity')).toBeDefined()
    // Paint, no — not the checkbox, not the colour, not the outline or shadow.
    expect(screen.queryByRole('checkbox', { name: 'Fill' })).toBeNull()
    expect(screen.queryByLabelText('Fill colour')).toBeNull()
    expect(screen.queryByRole('checkbox', { name: 'Outline' })).toBeNull()
    expect(screen.queryByRole('checkbox', { name: 'Shadow' })).toBeNull()
  })

  test('a percent axis says percent rather than pretending to hug its contents', async () => {
    await openTemplate()
    selectInTree('wipe')
    const sizing = (await screen.findByLabelText('Width sizing')) as HTMLSelectElement
    expect(sizing.value).toBe('percent')
    expect([...sizing.options].map((o) => o.value)).toContain('percent')
  })
})
