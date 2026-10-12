/**
 * Configure, tested for the things an operator needs when it is going wrong.
 *
 * The rail exists so the mode can grow; the diagnostics exist because every
 * number in them was already reported by a node and none of it was on a screen;
 * and the users page exists because the footer has been telling people to come
 * here since M2, to a page that did not exist.
 */

import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import { act, render, screen, fireEvent, waitFor, within } from '@testing-library/react'
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
    audio_devices: [
      { host: 'CoreAudio', name: 'BlackHole 2ch', channels: 2, supports_rate: true, default: false },
      { host: 'CoreAudio', name: 'MacBook Pro Speakers', channels: 2, supports_rate: true, default: true },
    ],
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
let configuredOutputs: Record<string, unknown>[] = []
let savedOutputConfig: Record<string, unknown> | null = null
let outputDefinitions: Record<string, unknown>[] = []
let layerPuts: { url: string; body: Record<string, unknown> }[] = []

beforeEach(() => {
  resetQuery()
  users = { users: [], groups: [] }
  created = null
  configuredOutputs = [{ id: 'wall', backend: 'ndi' }]
  savedOutputConfig = null
  outputDefinitions = []
  layerPuts = []
  vi.stubGlobal(
    'fetch',
    vi.fn(async (url: string, init?: RequestInit) => {
      const u = String(url)
      if (u === '/api/users' && init?.method === 'POST') {
        created = JSON.parse(String(init.body)) as Record<string, unknown>
        return new Response(JSON.stringify({ id: 'u1', granted_admin: true }), { status: 201 })
      }
      if (/\/outputs\/[^/]+$/.test(u) && init?.method === 'PUT') {
        layerPuts.push({ url: u, body: JSON.parse(String(init.body)) as Record<string, unknown> })
        return new Response(JSON.stringify({ ok: true }), { status: 200 })
      }
      if (u.endsWith('/output-config') && init?.method === 'PUT') {
        savedOutputConfig = JSON.parse(String(init.body)) as Record<string, unknown>
        return new Response(JSON.stringify({ delivered: true }), { status: 200 })
      }
      const body =
        u === '/api/nodes' ? { nodes: [NODE] }
        : u === '/api/screens' ? { screens: [] }
        : u === '/api/templates' ? { templates: [] }
        : u === '/api/users' ? users
        : u === '/api/stations' ? { stations: [{ id: 's1', name: 'Booth Mac', trusted: false }] }
        : u.endsWith('/output-config') ? { outputs: configuredOutputs, ack: null }
        : u.endsWith('/outputs') ? { outputs: outputDefinitions }
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

  await screen.findByText(/open access is enabled/i)
  // The consequence is stated BEFORE the button is pressed, not after.
  expect(screen.getByText(/that user becomes an Administrator/)).toBeDefined()
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
  await screen.findByText(/dan is now an Administrator/)
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

/**
 * Video on SDI, audio to a Dante Virtual Soundcard (ADR 0028). The picker is
 * driven by the node's own probe, and it only appears when there is audio to
 * send — a destination for nothing is a question with no consequence.
 */
test('an output’s audio can be sent to a sound device instead of the video', async () => {
  await paint()
  await screen.findByText('booth-1')
  for (const d of document.querySelectorAll('details.cfg-console__diag')) {
    ;(d as HTMLDetailsElement).open = true
  }
  await act(async () => {})

  // No audio yet, so nowhere to send it.
  expect(screen.queryByLabelText(/audio destination/)).toBeNull()

  await act(async () => {
    fireEvent.change(screen.getByLabelText('Output wall audio'), { target: { value: '2' } })
  })
  const dest = (await screen.findByLabelText('Output wall audio destination')) as HTMLSelectElement
  expect([...dest.options].map((o) => o.textContent)).toEqual([
    'Embedded in the video',
    'BlackHole 2ch · 2 ch',
    'MacBook Pro Speakers · 2 ch',
  ])

  await act(async () => {
    fireEvent.change(dest, { target: { value: 'BlackHole 2ch' } })
  })
  // Said out loud: the audio is NOT in the video any more, and two clocks are
  // now being held together. Matched on a contiguous run — the sentence is
  // broken up by the <b> around the device name.
  await screen.findByText(
    (_, element) => element?.tagName === 'P' && element.textContent?.includes('Audio leaves through BlackHole 2ch') === true,
  )
})

test('multiple outputs stay compact and only the selected output is editable', async () => {
  configuredOutputs = [
    { id: 'wall', backend: 'ndi', ndi_name: 'Wall', audio_channels: 0 },
    { id: 'stream', backend: 'ndi', ndi_name: 'Stream', audio_channels: 2 },
    { id: 'confidence', backend: 'headless', audio_channels: 0 },
  ]
  await paint()

  const navigator = await screen.findByRole('navigation', { name: 'Configured outputs' })
  expect(within(navigator).getAllByRole('button')).toHaveLength(3)
  expect(screen.getByLabelText('Output wall backend')).toBeDefined()
  expect(screen.queryByLabelText('Output stream backend')).toBeNull()

  fireEvent.click(within(navigator).getByRole('button', { name: /^streamNDI/ }))
  expect(screen.queryByLabelText('Output wall backend')).toBeNull()
  expect(screen.getByLabelText('Output stream backend')).toBeDefined()
})

test('hardware changes apply through the existing node output-config endpoint', async () => {
  configuredOutputs = [
    { id: 'wall', backend: 'ndi', ndi_name: 'Wall', audio_channels: 0 },
    { id: 'stream', backend: 'ndi', ndi_name: 'Stream', audio_channels: 2 },
  ]
  await paint()
  const navigator = await screen.findByRole('navigation', { name: 'Configured outputs' })
  fireEvent.click(within(navigator).getByRole('button', { name: /^streamNDI/ }))

  fireEvent.change(screen.getByLabelText('Output stream backend'), { target: { value: 'headless' } })
  expect(screen.getByLabelText('Unsaved output configuration')).toBeDefined()
  fireEvent.click(screen.getByRole('button', { name: 'Apply' }))

  await waitFor(() => expect(savedOutputConfig).toEqual({
    outputs: [
      { id: 'wall', backend: 'ndi', ndi_name: 'Wall', audio_channels: 0 },
      { id: 'stream', backend: 'headless', ndi_name: 'Stream', audio_channels: 2 },
    ],
    force: false,
  }))
})

/**
 * The workspace shows one output at a time, so the layer stack is now a single
 * component that the selection re-points rather than one per output. React
 * keeps mount-time state unless something tells it the identity changed — so
 * without that, selecting the second output would show the first one's layers
 * and write them to it.
 */
test('selecting another output shows that output’s layer stack, not the last one’s', async () => {
  configuredOutputs = [
    { id: 'wall', backend: 'ndi', ndi_name: 'Wall' },
    { id: 'stream', backend: 'ndi', ndi_name: 'Stream' },
  ]
  outputDefinitions = [
    { output_id: 'wall', mode: '1920x1080', layers: ['Lower Third', 'Ticker'] },
    {
      output_id: 'stream',
      mode: '1920x1080',
      layers: ['Bug'],
      color: { matrix: 'bt2020', range: 'full', key_range: 'narrow', key_mode: 'fill' },
    },
  ]
  await paint()

  const navigator = await screen.findByRole('navigation', { name: 'Configured outputs' })
  await screen.findByText('Lower Third')

  fireEvent.click(within(navigator).getByRole('button', { name: /^streamNDI/ }))
  await screen.findByText('Bug')
  expect(screen.queryByText('Lower Third')).toBeNull()
  expect(screen.queryByText('Ticker')).toBeNull()
  // And the colour with it — every control in the stack saves on change, so a
  // carried-over matrix is written to this output by the next click.
  expect((screen.getByLabelText('Matrix') as HTMLSelectElement).value).toBe('bt2020')
})

/**
 * Every colour control saves on change — and it saved the value from BEFORE
 * the change, because the handler read state its own update had not reached.
 * Each pick took effect one pick late, and the first one never.
 */
test('a colour pick saves what was picked, not what it replaced', async () => {
  configuredOutputs = [{ id: 'wall', backend: 'ndi', ndi_name: 'Wall' }]
  outputDefinitions = [{ output_id: 'wall', mode: '1920x1080', layers: ['Lower Third'] }]
  await paint()
  await screen.findByText('Lower Third')

  fireEvent.change(screen.getByLabelText('Matrix'), { target: { value: 'bt601' } })
  await waitFor(() => expect(layerPuts).toHaveLength(1))
  const colour = layerPuts[0]?.body.color as { matrix?: string } | undefined
  expect(colour?.matrix).toBe('bt601')
})

/**
 * The id field edits a DRAFT that the node has not been told about yet. The
 * layer stack belongs to the output the node already knows, so it must be
 * addressed by that id — a PUT to the typed one would upsert a second output
 * row for a thing that does not exist and push it straight to the node.
 */
test('a layer change addresses the output the node knows, not an unapplied rename', async () => {
  configuredOutputs = [{ id: 'wall', backend: 'ndi', ndi_name: 'Wall' }]
  outputDefinitions = [{ output_id: 'wall', mode: '1920x1080', layers: ['Lower Third', 'Ticker'] }]
  await paint()
  await screen.findByText('Lower Third')

  fireEvent.change(screen.getByLabelText('Output 1 id'), { target: { value: 'wall-renamed' } })
  fireEvent.click(screen.getByRole('button', { name: 'Remove Ticker' }))

  await waitFor(() => expect(layerPuts).toHaveLength(1))
  expect(layerPuts[0]?.url).toBe(`/api/nodes/${NODE.node_id}/outputs/wall`)
  expect(layerPuts[0]?.body.layers).toEqual(['Lower Third'])
})
