/**
 * Edit, tested for the things that were locked.
 *
 * The rundown rail rendered `rundowns[0]` and nothing else: one running order,
 * created once, unnameable and unswitchable, with no way to start next week's
 * without editing this week's. And the destination card put three selects side
 * by side in a 360px inspector, so every option read as "Fires with the tak".
 */

import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { Edit } from './Edit'
import { _reset as resetQuery } from '../../lib/useQuery'

const TEMPLATES = [{ id: 'tpl-1', name: 'Lower Third', head_rev: 'blake3:rev' }]
const PLAYLISTS = [{ id: 'pl-1', name: 'Amazing Grace', items: 0, mode: 'cue' }]

let rundowns: { id: string; name: string }[]
let sent: { url: string; method: string; body: unknown }[]

const rundownDetail = (id: string) => ({
  id,
  name: rundowns.find((r) => r.id === id)?.name ?? '?',
  playlists: [],
  cursor: null,
})

beforeEach(() => {
  resetQuery()
  rundowns = [
    { id: 'rd-1', name: 'Sunday 09:00' },
    { id: 'rd-2', name: 'Sunday 11:00' },
  ]
  sent = []

  vi.stubGlobal(
    'fetch',
    vi.fn(async (url: RequestInfo | URL, init?: RequestInit) => {
      const u = String(url)
      const method = init?.method ?? 'GET'
      const body = init?.body ? JSON.parse(String(init.body)) : null
      if (method !== 'GET') sent.push({ url: u, method, body })

      const json = (b: unknown, status = 200) => new Response(JSON.stringify(b), { status })

      if (method === 'PATCH' && u.startsWith('/api/rundowns/')) {
        const id = u.split('/').pop()!
        rundowns = rundowns.map((r) => (r.id === id ? { ...r, name: String((body as { name: string }).name) } : r))
        return json(rundownDetail(id))
      }
      if (method === 'POST' && u === '/api/rundowns') {
        rundowns = [...rundowns, { id: 'rd-3', name: String((body as { name: string }).name) }]
        return json({ id: 'rd-3' }, 201)
      }
      if (u === '/api/templates') return json({ templates: TEMPLATES })
      if (u === '/api/items') return json({ items: [] })
      if (u === '/api/playlists') return json({ playlists: PLAYLISTS })
      if (u === '/api/rundowns') return json({ rundowns })
      if (u.startsWith('/api/rundowns/')) return json(rundownDetail(u.split('/').pop()!))
      if (u === '/api/screens') return json({ screens: [] })
      if (u === '/api/nodes') return json({ nodes: [] })
      return json({})
    }),
  )
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

const paint = async () => {
  render(<Edit />)
  await act(async () => {})
}

test('the rundown is a choice, not whichever one happened to be first', async () => {
  await paint()
  const pick = (await screen.findByLabelText('Rundown')) as HTMLSelectElement
  expect([...pick.options].map((o) => o.textContent)).toEqual(['Sunday 09:00', 'Sunday 11:00'])

  await act(async () => {
    fireEvent.change(pick, { target: { value: 'rd-2' } })
  })
  // The rail follows the choice rather than snapping back to the first.
  await waitFor(() =>
    expect((screen.getByLabelText('Rundown') as HTMLSelectElement).value).toBe('rd-2'),
  )
})

test('a rundown can be renamed where it is read', async () => {
  await paint()
  await screen.findByLabelText('Rundown')

  fireEvent.click(screen.getByLabelText('Rename this rundown'))
  const field = await screen.findByLabelText('Rundown name')
  fireEvent.change(field, { target: { value: 'Christmas Eve' } })
  await act(async () => {
    fireEvent.blur(field)
  })

  expect(sent.find((c) => c.method === 'PATCH')?.body).toEqual({ name: 'Christmas Eve' })
  await waitFor(() =>
    expect(
      [...(screen.getByLabelText('Rundown') as HTMLSelectElement).options].map((o) => o.textContent),
    ).toContain('Christmas Eve'),
  )
})

test('a new rundown is made, selected, and opened for naming in one go', async () => {
  await paint()
  await screen.findByLabelText('Rundown')

  await act(async () => {
    fireEvent.click(screen.getByLabelText('New rundown'))
  })
  // Made, then selected — a new running order you cannot see is not made.
  await waitFor(() => expect(screen.getByLabelText('Rundown name')).toBeDefined())
})

test('deleting a rundown asks first, and says the playlists survive', async () => {
  const confirm = vi.fn((_msg?: string) => false)
  vi.stubGlobal('confirm', confirm)
  await paint()
  await screen.findByLabelText('Rundown')

  fireEvent.click(screen.getByLabelText('Delete this rundown'))
  expect(confirm).toHaveBeenCalledOnce()
  expect(String(confirm.mock.calls[0]?.[0])).toMatch(/playlists stay/)
  // Refused, so nothing was sent.
  expect(sent.some((c) => c.method === 'DELETE')).toBe(false)
})

test('a destination stacks its controls in one column, one per line', async () => {
  await paint()
  fireEvent.click(await screen.findByText('New item'))
  await screen.findByText('Where it goes')

  // Three selects sharing a 360px inspector gave each about 100px, so "Fires
  // with the take" rendered as "Fires with the tak" and the other options had
  // to be guessed at. The fix is structural — every control is a sibling in
  // one column — and the CSS that stacks them cannot be read in jsdom.
  const rows = document.querySelectorAll('.edit-console__dest-card .edit-console__dest-row')
  expect(rows.length).toBe(1)
  expect(rows[0]!.querySelectorAll(':scope > .c-formgroup').length).toBeGreaterThanOrEqual(4)

  // And the remove control left the control row for the card's own header, so
  // it stops competing with a select for horizontal room.
  expect(document.querySelector('.edit-console__dest-head')).toBeTruthy()
})
