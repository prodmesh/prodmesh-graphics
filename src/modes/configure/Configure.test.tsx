/**
 * Configure, tested for the things an operator needs when it is going wrong.
 *
 * The rail exists so the mode can grow; the diagnostics exist because every
 * number in them was already reported by a node and none of it was on a screen;
 * and the users page exists because the footer has been telling people to come
 * here since M2, to a page that did not exist.
 */

import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import { act, render, screen, fireEvent, waitFor } from '@testing-library/react'
import { Configure } from './Configure'
import { _reset as resetQuery } from '../../lib/useQuery'

const NODE = {
  node_id: 'C7A9A9ACFB4CACE6F520D7AB0B',
  name: 'booth-1',
  reachable: true,
  adopted: true,
  version: '0.1.0',
  assignment_gen: 2,
  last_seen: 1_755_691_200_000,
  address: '10.0.1.42',
  api_port: 8520,
  status_url: 'http://10.0.1.42:8520/status',
  preview_url: 'http://10.0.1.42:8521/',
  capabilities: {
    max_layers: 8,
    scene_reader_major: 1,
    decoders: ['prores'],
    // What the node's own probe found (ADR 0024). This stood empty for four
    // milestones, so Configure's device pickers offered "no card reported" on
    // a machine with eight connectors in it.
    outputs: [
      {
        device: 'DeckLink Quad (1)',
        kind: 'decklink',
        index: 0,
        modes: ['1080p59.94', '1080i59.94'],
        supports_key_fill: false,
        has_genlock: true,
        genlock_locked: false,
        audio_pairs: 8,
      },
      {
        device: 'DeckLink Quad (2)',
        kind: 'decklink',
        index: 1,
        modes: ['1080p59.94', '1080i59.94'],
        supports_key_fill: true,
        has_genlock: true,
        genlock_locked: false,
        audio_pairs: 8,
      },
    ],
  },
  state: {
    outputs: [
      { id: 'wall', fps: 59.94, late_frames: 3, layers: [{ name: 'Graphics', occupant: null }] },
    ],
  },
}

let users: { users: unknown[]; groups: unknown[] } = { users: [], groups: [] }
let created: Record<string, unknown> | null = null

beforeEach(() => {
  resetQuery()
  users = { users: [], groups: [] }
  created = null
  vi.stubGlobal(
    'fetch',
    vi.fn(async (url: string, init?: RequestInit) => {
      const u = String(url)
      if (u === '/api/users' && init?.method === 'POST') {
        created = JSON.parse(String(init.body)) as Record<string, unknown>
        return new Response(JSON.stringify({ id: 'u1', granted_admin: true }), { status: 201 })
      }
      const body =
        u === '/api/nodes' ? { nodes: [NODE] }
        : u === '/api/screens' ? { screens: [] }
        : u === '/api/templates' ? { templates: [] }
        : u === '/api/users' ? users
        : u === '/api/stations' ? { stations: [{ id: 's1', name: 'Booth Mac', trusted: false }] }
        : u.endsWith('/outputs') ? { outputs: [] }
        : {}
      return new Response(JSON.stringify(body), { status: 200 })
    }),
  )
})

afterEach(() => vi.unstubAllGlobals())

const paint = async () => {
  render(<Configure />)
  await act(async () => {})
}

test('the rail switches sections, and starts on the nodes', async () => {
  await paint()
  await screen.findByText('booth-1')

  fireEvent.click(screen.getByRole('button', { name: /Users & stations/ }))
  await screen.findByRole('region', { name: 'Stations' })
  // And the node section is gone rather than merely scrolled past.
  expect(screen.queryByText('booth-1')).toBeNull()
})

test('a node shows where it is and how it is doing, and links to its own page', async () => {
  await paint()
  await screen.findByText('booth-1')

  // Present but folded away: this is for when something is wrong, not for
  // every glance at the page.
  const diag = document.querySelector('details.cfg-console__diag') as HTMLDetailsElement
  expect(diag).toBeTruthy()
  diag.open = true
  await act(async () => {})

  expect(screen.getByText('10.0.1.42')).toBeDefined()
  expect(screen.getByText('gen 2 acked')).toBeDefined()
  // The late-frame count is the number that matters, and it is per output.
  expect(screen.getByText('59.94 fps · 3 late')).toBeDefined()

  const link = screen.getByRole('link', { name: /10\.0\.1\.42:8520\/status/ })
  expect(link.getAttribute('href')).toBe('http://10.0.1.42:8520/status')
})

test('the users page says an open server is open, and names what adding a user does', async () => {
  await paint()
  fireEvent.click(screen.getByRole('button', { name: /Users & stations/ }))

  await screen.findByText(/anyone on this network can operate this server/)
  // The consequence is stated BEFORE the button is pressed, not after.
  expect(screen.getByText(/makes that user an Administrator/)).toBeDefined()
})

test('adding the first user reports that it was made an administrator', async () => {
  await paint()
  fireEvent.click(screen.getByRole('button', { name: /Users & stations/ }))
  await screen.findByLabelText('New username')

  fireEvent.change(screen.getByLabelText('New username'), { target: { value: 'dan' } })
  fireEvent.change(screen.getByLabelText('New user PIN'), { target: { value: '1234' } })
  fireEvent.click(screen.getByRole('button', { name: 'Add user' }))

  await waitFor(() => expect(created).toEqual({ username: 'dan', pin: '1234', groups: [] }))
  // A permission nobody knows was granted is a permission nobody audits.
  await screen.findByText(/dan is an Administrator/)
})

/**
 * The whole point of the sentence above it: creating the first user turns
 * permissions on. Until the shell hears about it, the footer of this very page
 * still says the server is open and the header still offers no way in — which
 * is exactly how somebody ends up locked out of a server they just secured.
 */
test('adding the first user tells the shell that permissions are now on', async () => {
  const heard: Event[] = []
  const listen = (e: Event) => heard.push(e)
  window.addEventListener('pmg:auth-changed', listen)

  await paint()
  fireEvent.click(screen.getByRole('button', { name: /Users & stations/ }))
  await screen.findByLabelText('New username')
  fireEvent.change(screen.getByLabelText('New username'), { target: { value: 'dan' } })
  fireEvent.change(screen.getByLabelText('New user PIN'), { target: { value: '1234' } })
  fireEvent.click(screen.getByRole('button', { name: 'Add user' }))

  await waitFor(() => expect(heard.length).toBe(1))
  window.removeEventListener('pmg:auth-changed', listen)
})

test('a PIN under four digits does not offer the button', async () => {
  await paint()
  fireEvent.click(screen.getByRole('button', { name: /Users & stations/ }))
  await screen.findByLabelText('New username')

  fireEvent.change(screen.getByLabelText('New username'), { target: { value: 'dan' } })
  fireEvent.change(screen.getByLabelText('New user PIN'), { target: { value: '12' } })
  expect((screen.getByRole('button', { name: 'Add user' }) as HTMLButtonElement).disabled).toBe(true)
})

/**
 * The output editor is driven by what the NODE reports, and for four
 * milestones the node reported nothing: `capabilities()` returned an empty
 * list behind a comment saying a real probe would land later. Configure offered
 * "no card reported" on a machine with a Quad 2 in it.
 */
test('the device pickers come from the node’s own probe, and the modes come from the connector', async () => {
  await paint()
  await screen.findByText('booth-1')
  // Diagnostics and Output hardware are both folded away — this is for when
  // something is wrong, not for every glance at the page.
  for (const d of document.querySelectorAll('details.cfg-console__diag')) {
    ;(d as HTMLDetailsElement).open = true
  }
  await act(async () => {})

  await act(async () => {
    fireEvent.change(screen.getByLabelText(/backend/), { target: { value: 'decklink' } })
  })

  const fill = (await screen.findByLabelText('Output wall device')) as HTMLSelectElement
  expect([...fill.options].map((o) => o.textContent)).toEqual([
    'DeckLink Quad (1) · genlock',
    'DeckLink Quad (2) · key+fill · genlock',
  ])

  // Only what THIS connector says it can do. Offering a mode the card refuses
  // is a choice that fails silently until the output is armed.
  const mode = screen.getByLabelText('Output wall mode') as HTMLSelectElement
  expect([...mode.options].map((o) => o.textContent)).toEqual([
    'Choose a mode',
    '1080p59.94',
    '1080i59.94',
  ])
})
