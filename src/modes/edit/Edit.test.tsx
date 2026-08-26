/**
 * Edit, tested for the things that were locked.
 *
 * The rundown rail rendered `rundowns[0]` and nothing else: one running order,
 * created once, unnameable and unswitchable, with no way to start next week's
 * without editing this week's. And the destination card put three selects side
 * by side in a 360px inspector, so every option read as "Fires with the tak".
 */

import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { Edit } from './Edit'
import { _reset as resetQuery } from '../../lib/useQuery'

const TEMPLATES = [
  { id: 'tpl-1', name: 'Lower Third', head_rev: 'blake3:rev' },
  { id: 'tpl-2', name: 'LED wall', head_rev: 'blake3:rev2' },
]

/** Two templates that want DIFFERENT fields — the ADR 0018 case. */
const MANIFESTS: Record<string, { fields: { key: string; type: string; label?: string; required?: boolean }[] }> = {
  'tpl-1': { fields: [{ key: 'lyric', type: 'text', label: 'Lyric', required: true }] },
  'tpl-2': { fields: [{ key: 'name', type: 'text', label: 'Line' }, { key: 'title', type: 'text' }] },
}
const PLAYLISTS = [{ id: 'pl-1', name: 'Amazing Grace', items: 2, mode: 'cue', tags: ['Hymn'] }]

const entry = (id: string, label: string, entryId: string) => ({
  id,
  entry_id: entryId,
  take_id: null,
  label,
  template_id: 'tpl-1',
  template_rev: 'blake3:rev',
  data: {},
  destination: { layer: 'Lower Third' },
  destinations: [],
  section: null,
  updated_at: 0,
})

// One song: a verse and a chorus, with the chorus arranged twice (ADR 0026).
const CHORUS = entry('it-2', 'Chorus', 'e-2')
const ARRANGEMENT = { id: 'arr-1', name: 'Short', groups: ['Chorus'] }
const PLAYLIST = {
  id: 'pl-1',
  name: 'Amazing Grace',
  mode: 'cue',
  tags: ['Hymn'],
  arrangement: null as string | null,
  arrangements: [ARRANGEMENT],
  groups: [
    { name: 'Verse 1', slides: 1 },
    { name: 'Chorus', slides: 1 },
  ],
  ungrouped: [] as string[],
  items: [entry('it-1', 'Verse 1', 'e-1'), CHORUS, { ...CHORUS, entry_id: 'e-3' }],
}

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
      if (u.startsWith('/api/templates/')) {
        const id = u.split('/')[3]!
        return json({ id, name: id, head: { manifest: MANIFESTS[id] ?? { fields: [] } } })
      }
      if (u === '/api/templates') return json({ templates: TEMPLATES })
      if (u.startsWith('/api/items?playlist=')) return json({ items: [PLAYLIST.items[0], CHORUS] })
      if (u.startsWith('/api/items')) return json({ items: [] })
      if (u === '/api/tags') return json({ tags: [{ tag: 'Hymn', playlists: 1 }] })
      if (u.startsWith('/api/playlists/pl-1?arrangement=')) {
        return json({ ...PLAYLIST, arrangement: 'arr-1', items: [CHORUS] })
      }
      if (u === '/api/playlists/pl-1') return json(PLAYLIST)
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

/** An item belongs to a playlist, so a new one needs one open (ADR 0026). */
const openPlaylist = async () => {
  // The name is in the rail AND in the rundown's "add a playlist" list.
  fireEvent.click((await screen.findAllByText('Amazing Grace'))[0]!)
  await screen.findByLabelText('Playlist name')
}

test('a destination stacks its controls in one column, one per line', async () => {
  await paint()
  await openPlaylist()
  fireEvent.click(screen.getByText('New item'))
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

// ---------------------------------------------------------------------------
// The library. ADR 0026.

test('the rail filters by name and by tag, and there is no All items list', async () => {
  await paint()
  await screen.findAllByText('Amazing Grace')

  // The list of every Item is gone: at a hundred songs of thirty slides it is
  // thousands of rows, which is not a way to find anything.
  expect(screen.queryByText('All items')).toBeNull()
  expect(screen.getByText('Find an item…')).toBeDefined()

  fireEvent.change(screen.getByLabelText('Filter playlists'), { target: { value: 'nothing' } })
  await waitFor(() => expect(screen.getByText(/Nothing matches/)).toBeDefined())

  fireEvent.change(screen.getByLabelText('Filter playlists'), { target: { value: 'amaz' } })
  await waitFor(() => expect(screen.getAllByText('Amazing Grace').length).toBeGreaterThan(0))

  // The tag came from the server's own list of what has been typed. Scoped to
  // the filter group — the playlist row's meta line names its tags too.
  const chip = within(screen.getByRole('group', { name: 'Filter by tag' })).getByRole('button')
  fireEvent.click(chip)
  expect(chip.getAttribute('aria-pressed')).toBe('true')
})

test('a repeated item is one item in several places, and each place says so', async () => {
  await paint()
  await openPlaylist()

  // Three entries, two of them the same Item — and that is the point: a typo
  // fixed once is fixed everywhere it appears.
  const rows = document.querySelectorAll('.edit-console__entry')
  expect(rows.length).toBe(3)
  expect(screen.getByText('1 of 2')).toBeDefined()
  expect(screen.getByText('2 of 2')).toBeDefined()

  // Removing one place leaves the other singing.
  const removals = screen.getAllByLabelText(/Remove Chorus from the arrangement/)
  expect(removals.length).toBe(2)
})

test('a new item needs a playlist, because it belongs to one', async () => {
  await paint()
  await screen.findByText('Find an item…')
  // Nothing is open: an Item created into nowhere is content that exists and
  // cannot be reached.
  expect((screen.getByText('New item').closest('button') as HTMLButtonElement).disabled).toBe(true)

  await openPlaylist()
  expect((screen.getByText('New item').closest('button') as HTMLButtonElement).disabled).toBe(false)
})

test('the playlist carries the tags, and one can be added by typing it', async () => {
  await paint()
  await openPlaylist()

  expect(screen.getByLabelText('Remove tag Hymn')).toBeDefined()
  const add = screen.getByLabelText('Add a tag')
  fireEvent.change(add, { target: { value: 'Christmas' } })
  await act(async () => {
    fireEvent.keyDown(add, { key: 'Enter' })
  })

  const put = sent.find((c) => c.method === 'PUT' && c.url.endsWith('/tags'))
  expect(put?.body).toEqual({ tags: ['Hymn', 'Christmas'] })
})

// ---------------------------------------------------------------------------
// Named arrangements. ADR 0027.

test('a named order is a list of GROUPS, and the slides go read-only', async () => {
  await paint()
  await openPlaylist()

  // Main is where slides are edited, so its controls are live.
  expect((screen.getAllByLabelText(/^Section for /)[0] as HTMLInputElement).disabled).toBe(false)

  await act(async () => {
    fireEvent.change(screen.getByLabelText('Arrangement'), { target: { value: 'arr-1' } })
  })

  // The order is groups, not slides: it holds names, which is what makes it
  // follow Main rather than becoming a second copy of the song.
  await screen.findByLabelText('Arrangement name')
  expect(screen.getByRole('list', { name: 'Group order' })).toBeDefined()
  expect(screen.getByLabelText('Remove Chorus at 1')).toBeDefined()

  // And every way of editing a slide from in here is shut off — editing one
  // would need this order to own it, and then Main and its arrangements could
  // disagree about the words.
  await waitFor(() =>
    expect((screen.getAllByLabelText(/^Section for /)[0] as HTMLInputElement).disabled).toBe(true),
  )
  expect(screen.queryByText('Arrange again')).toBeNull()
})

test('adding a group sends the whole order, by name', async () => {
  await paint()
  await openPlaylist()
  await act(async () => {
    fireEvent.change(screen.getByLabelText('Arrangement'), { target: { value: 'arr-1' } })
  })
  await screen.findByLabelText('Arrangement name')

  await act(async () => {
    fireEvent.click(screen.getByRole('button', { name: /Verse 1/ }))
  })
  const put = sent.find((c) => c.method === 'PUT' && c.url === '/api/arrangements/arr-1')
  expect(put?.body).toEqual({ groups: ['Chorus', 'Verse 1'] })
})

test('a song with no named groups is told why, and offered nothing to press', async () => {
  const groups = PLAYLIST.groups
  PLAYLIST.groups = []
  try {
    await paint()
    await openPlaylist()
    await screen.findByText(/Add a Section to each group/)
    expect(screen.queryByLabelText('Arrangement')).toBeNull()
  } finally {
    PLAYLIST.groups = groups
  }
})

/**
 * One Item, several destinations, each rendering it with its OWN template
 * (ADR 0018) — and those templates legitimately want different fields.
 *
 * The editor used to show only the FIRST destination's fields, so a wall
 * template asking for a line the lower third does not declare was invisible,
 * and the Item could not be saved from the screen meant to author it.
 */
test('the form shows every destination’s fields, and says which want what', async () => {
  await paint()
  await openPlaylist()
  fireEvent.click(screen.getByText('New item'))
  await screen.findByText('Where it goes')

  // One destination: just its own field, and no noise about where it came from.
  await waitFor(() => expect(screen.getByLabelText(/^Lyric/)).toBeDefined())
  expect(screen.queryByText(/only/)).toBeNull()

  fireEvent.click(screen.getByText('Add a destination'))
  const pickers = screen.getAllByLabelText(/Rendered by|^Destination \d+ screen$/)
  void pickers
  const rendered = document.querySelectorAll('.edit-console__dest-card select')
  // The second card's template picker is the last "Rendered by" select.
  const templateSelects = [...rendered].filter((s) =>
    [...(s as HTMLSelectElement).options].some((o) => o.textContent === 'LED wall'),
  ) as HTMLSelectElement[]
  await act(async () => {
    fireEvent.change(templateSelects[templateSelects.length - 1]!, { target: { value: 'tpl-2' } })
  })

  // Now BOTH templates' fields are here, each saying which destination wants
  // it — the wall's two, and the lower third's one.
  await waitFor(() => expect(screen.getByLabelText(/^Line · LED wall only/)).toBeDefined())
  expect(screen.getByLabelText(/^title · LED wall only/)).toBeDefined()
  expect(screen.getByLabelText(/^Lyric · required · Lower Third only/)).toBeDefined()
})
