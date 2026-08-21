/**
 * The timeline panel's key inspector, and the curve editor inside it.
 *
 * The format has carried arbitrary cubic Béziers since v1 and `pmg-anim` has
 * evaluated them all along; the editor offered four names and showed anything
 * else as "Custom", so a designer could pick a curve and never draw one. These
 * tests are about the drawing: that a drag commits a curve, that it commits on
 * RELEASE rather than per pointermove, and that an overshoot survives — the
 * anticipation curve is the reason to hand-draw one at all, and clamping y to
 * [0,1] would quietly delete it.
 */

import { describe, expect, test, vi } from 'vitest'
import { render, screen, fireEvent, act } from '@testing-library/react'
import { useRef } from 'react'
import { TimelinePanel } from './TimelinePanel'
import type { DesignSession } from './useDesign'

const DOC = {
  timelines: [
    {
      name: 'In',
      duration_ms: 1000,
      tracks: [
        {
          target: 'band/transform.translate.x',
          keys: [
            { t: 0, v: -200 },
            { t: 1000, v: 0 },
          ],
        },
      ],
    },
  ],
}

/** The session's `setKey`, typed so a test can read the ease it was handed. */
const setKeyMock = () =>
  vi.fn(
    (_timeline: string, _node: string, _prop: string, _t: number, _v: number, _ease: string) =>
      '{}',
  )
type SetKey = ReturnType<typeof setKeyMock>

function harness(setKey: SetKey) {
  const session = {
    setKey,
    addTimeline: vi.fn(() => '{}'),
    deleteKey: vi.fn(() => '{}'),
    retimeKey: vi.fn(() => '{}'),
  } as unknown as DesignSession

  function Host() {
    const ref = useRef<DesignSession | null>(session)
    return (
      <TimelinePanel
        session={ref}
        ready
        doc={DOC}
        selected="band"
        timeline="In"
        tMs={0}
        playing={false}
        onTogglePlay={() => {}}
        onPickTimeline={() => {}}
        onScrub={() => {}}
        onEdit={() => {}}
      />
    )
  }
  return Host
}

/** Click the first key diamond, which is what opens the key inspector. */
const selectFirstKey = () => {
  const key = document.querySelector('.tl__key')!
  fireEvent.pointerDown(key)
  fireEvent.click(key)
}

/**
 * Drag a curve handle to a point in curve space.
 *
 * The SVG has no layout in jsdom, so its bounding box is zeroes; the component
 * divides by `r.width`, which would be a NaN. The box is stubbed to the size
 * the browser gives it, which is the only thing this test needs jsdom to
 * pretend about.
 */
const dragHandle = (i: number, x: number, y: number) => {
  const svg = document.querySelector('.curve__svg') as SVGSVGElement
  const BOX = 176 // PAD * 2 + SIZE
  svg.getBoundingClientRect = () =>
    ({ left: 0, top: 0, width: BOX, height: BOX, right: BOX, bottom: BOX, x: 0, y: 0 }) as DOMRect
  const client = (cx: number, cy: number) => ({
    clientX: 28 + cx * 120,
    clientY: 28 + (1 - cy) * 120,
  })
  const handle = document.querySelectorAll('.curve__handle')[i]!
  fireEvent.pointerDown(handle, client(0.5, 0.5))
  fireEvent.pointerMove(svg, client(x, y))
  return () => fireEvent.pointerUp(svg, client(x, y))
}

describe('the curve editor', () => {
  test('a Linear key opens at the diagonal — linear IS a cubic, so the first drag is continuous', async () => {
    const setKey = setKeyMock()
    const Host = harness(setKey)
    render(<Host />)
    await act(async () => {})
    selectFirstKey()

    expect(screen.getByText('cubic-bezier(0.33, 0.33, 0.67, 0.67)')).toBeDefined()
  })

  test('a drag commits ON RELEASE — one undo entry per drag, not one per pixel', async () => {
    const setKey = setKeyMock()
    const Host = harness(setKey)
    render(<Host />)
    await act(async () => {})
    selectFirstKey()

    const release = dragHandle(0, 0.1, 0.9)
    // Mid-drag the curve follows the pointer...
    expect(screen.getByText(/cubic-bezier\(0\.10, 0\.90/)).toBeDefined()
    // ...and nothing has been written to the document yet.
    expect(setKey).not.toHaveBeenCalled()

    release()
    expect(setKey).toHaveBeenCalledTimes(1)
    const ease = JSON.parse(setKey.mock.calls[0]![5]) as { cubic: number[] }
    expect(ease.cubic[0]).toBeCloseTo(0.1, 2)
    expect(ease.cubic[1]).toBeCloseTo(0.9, 2)
  })

  test('an overshoot past the target survives — that is the anticipation curve', async () => {
    const setKey = setKeyMock()
    const Host = harness(setKey)
    render(<Host />)
    await act(async () => {})
    selectFirstKey()

    dragHandle(1, 0.7, 1.4)()
    const ease = JSON.parse(setKey.mock.calls[0]![5]) as { cubic: number[] }
    expect(ease.cubic[3]).toBeGreaterThan(1)
  })

  test('x is clamped to the time axis — a control point outside it is not a function', async () => {
    const setKey = setKeyMock()
    const Host = harness(setKey)
    render(<Host />)
    await act(async () => {})
    selectFirstKey()

    dragHandle(0, 3.0, 0.5)()
    const ease = JSON.parse(setKey.mock.calls[0]![5]) as { cubic: number[] }
    expect(ease.cubic[0]).toBeLessThanOrEqual(1)
    expect(ease.cubic[0]).toBeGreaterThanOrEqual(0)
  })

  test('the keyboard reaches the same curve', async () => {
    const setKey = setKeyMock()
    const Host = harness(setKey)
    render(<Host />)
    await act(async () => {})
    selectFirstKey()

    fireEvent.keyDown(screen.getByRole('slider', { name: 'Curve handle 1' }), { key: 'ArrowRight' })
    expect(setKey).toHaveBeenCalledTimes(1)
    const ease = JSON.parse(setKey.mock.calls[0]![5]) as { cubic: number[] }
    expect(ease.cubic[0]).toBeCloseTo(0.35, 2)
  })
})
