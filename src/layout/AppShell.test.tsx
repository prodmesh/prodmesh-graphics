/**
 * Identity in the shell (ADR 0025).
 *
 * The bug this was written for: creating the first user turned permissions on
 * and there was nowhere to sign in — the console had a lock with no keyhole.
 *
 * So the claims worth defending are about being FINDABLE and about being
 * honest: the way in is in the chrome whenever the server has users, a refusal
 * opens it naming the authority that was missing, an open server does not offer
 * a door onto a wall, and signed-out is stated as a state rather than left to be
 * discovered by pressing something.
 */

import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'

import { AppShell } from './AppShell'
import { post } from '../lib/api'
import { _reset as resetQuery } from '../lib/useQuery'

let me: Record<string, unknown>
let calls: { url: string; init?: RequestInit }[]
let loginAnswer: { status: number; body: Record<string, unknown> }

const signedOut = () => ({ user: null, station: null, permissions: [], unconfigured: false })

beforeEach(() => {
  resetQuery()
  window.localStorage.clear()
  calls = []
  me = signedOut()
  loginAnswer = { status: 200, body: { token: 'session-token' } }

  vi.stubGlobal(
    'fetch',
    vi.fn(async (url: string, init?: RequestInit) => {
      calls.push({ url: String(url), init })
      const u = String(url)
      if (u === '/api/stations/register') {
        return new Response(JSON.stringify({ id: 'st1', name: 'x', token: 'station-token' }), { status: 201 })
      }
      if (u === '/api/auth/login') {
        return new Response(JSON.stringify(loginAnswer.body), { status: loginAnswer.status })
      }
      if (u === '/api/auth/me') return new Response(JSON.stringify(me), { status: 200 })
      if (u === '/api/nodes') return new Response(JSON.stringify({ nodes: [] }), { status: 200 })
      return new Response(JSON.stringify({}), { status: 200 })
    }),
  )
})

afterEach(() => vi.unstubAllGlobals())

const paint = async () => {
  render(
    <MemoryRouter initialEntries={['/show']}>
      <AppShell />
    </MemoryRouter>,
  )
  await act(async () => {})
}

test('this browser registers as a station once, and carries it on every request', async () => {
  await paint()
  await waitFor(() => expect(window.localStorage.getItem('pmg.station')).toBe('station-token'))

  const meCall = calls.find((c) => c.url === '/api/auth/me')
  const sent = meCall?.init?.headers as Record<string, string> | undefined
  expect(sent?.['x-prodmesh-station']).toBe('station-token')

  // Second mount: the station already exists, and registering again would make
  // one machine two rows in the trusted-station list.
  calls.length = 0
  resetQuery()
  await paint()
  expect(calls.some((c) => c.url === '/api/stations/register')).toBe(false)
})

test('a server with users offers a way in, and says the station is only watching', async () => {
  await paint()
  await screen.findByRole('button', { name: 'Sign in' })
  await screen.findByText(/this station can watch, not change/i)
})

test('an open server offers no sign in, because there is nobody to sign in as', async () => {
  me = { ...signedOut(), unconfigured: true }
  await paint()
  await screen.findByText(/anyone on this network can operate this server/)
  expect(screen.queryByRole('button', { name: 'Sign in' })).toBeNull()
})

/**
 * The lockout that happened for real: an account in no group, so enforcement is
 * on and nobody can grant anything. Signing in cannot fix it, so offering that
 * as the answer sends the operator round a loop with no exit.
 */
test('a server that has lost its administrator says so, and names the command', async () => {
  me = { ...signedOut(), no_administrator: true }
  await paint()
  await screen.findByText(/No account can manage users/)
  expect(screen.getByText('npm run grant-admin -- <username>')).toBeDefined()
})

test('a refused action opens the way in, and names the authority', async () => {
  await paint()
  await screen.findByRole('button', { name: 'Sign in' })

  // What a 401 from the server looks like arriving through the write path.
  vi.mocked(fetch).mockImplementationOnce(
    async () =>
      new Response(
        JSON.stringify({
          error: 'permission_required',
          permission: 'graphics.users.manage',
          label: 'Sign in to do that.',
        }),
        { status: 401 },
      ),
  )
  await act(async () => {
    await post('/api/users', {}).catch(() => {})
  })

  await screen.findByRole('region', { name: 'Sign in' })
  expect(screen.getByLabelText('Username')).toBeDefined()
})

test('signing in keeps the session and shows who is signed in', async () => {
  await paint()
  fireEvent.click(await screen.findByRole('button', { name: 'Sign in' }))

  fireEvent.change(screen.getByLabelText('Username'), { target: { value: 'jbeale' } })
  fireEvent.change(screen.getByLabelText('PIN'), { target: { value: '1234' } })

  me = { user: { id: 'u1', username: 'jbeale' }, station: null, permissions: ['*'], unconfigured: false }
  await act(async () => {
    fireEvent.click(screen.getByRole('button', { name: /^Sign in$/ }))
  })

  await waitFor(() => expect(window.localStorage.getItem('pmg.session')).toBe('session-token'))
  await screen.findByText('JBEALE')
  // And the band gets out of the way rather than sitting over the console.
  expect(screen.queryByRole('region', { name: 'Sign in' })).toBeNull()
})

test('a wrong PIN is a sentence, and the PIN box is cleared', async () => {
  await paint()
  fireEvent.click(await screen.findByRole('button', { name: 'Sign in' }))
  fireEvent.change(screen.getByLabelText('Username'), { target: { value: 'jbeale' } })
  fireEvent.change(screen.getByLabelText('PIN'), { target: { value: '9999' } })

  loginAnswer = { status: 401, body: { error: 'bad_credentials', label: 'That username and PIN don’t match.' } }
  await act(async () => {
    fireEvent.click(screen.getByRole('button', { name: /^Sign in$/ }))
  })

  await screen.findByText(/don’t match/)
  expect((screen.getByLabelText('PIN') as HTMLInputElement).value).toBe('')
  expect(window.localStorage.getItem('pmg.session')).toBeNull()
})

test('signing out drops the token before telling the server, so it works when the server does not', async () => {
  me = { user: { id: 'u1', username: 'jbeale' }, station: null, permissions: ['*'], unconfigured: false }
  window.localStorage.setItem('pmg.session', 'session-token')
  await paint()
  await screen.findByText('JBEALE')

  vi.mocked(fetch).mockImplementation((async (url: RequestInfo | URL) => {
    if (String(url) === '/api/auth/logout') throw new Error('unreachable')
    me = signedOut()
    return new Response(JSON.stringify(String(url) === '/api/auth/me' ? me : {}), { status: 200 })
  }) as typeof fetch)

  await act(async () => {
    fireEvent.click(screen.getByRole('button', { name: 'Sign out' }))
  })
  expect(window.localStorage.getItem('pmg.session')).toBeNull()
  await screen.findByRole('button', { name: 'Sign in' })
})
