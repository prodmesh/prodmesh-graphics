import { describe, expect, test } from 'vitest'
import { render, screen } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import { App } from './App'

const at = (path: string) =>
  render(
    <MemoryRouter initialEntries={[path]}>
      <App />
    </MemoryRouter>,
  )

describe('the four modes', () => {
  // The mode is announced by the header tab (aria-current) and the footer's
  // mode tag — the console has no page headings, the same as the design it
  // transcribes (ADR 0020).
  test.each([
    ['/configure', 'Configure', 'CONFIGURE MODE'],
    ['/design', 'Design', 'DESIGN MODE'],
    ['/edit', 'Edit', 'EDIT MODE'],
    ['/show', 'Show', 'SHOW MODE'],
  ])('%s renders %s', (path, tab, footer) => {
    at(path)
    const link = screen.getByRole('link', { name: tab })
    expect(link.getAttribute('aria-current')).toBe('page')
    expect(screen.getByText(footer)).toBeDefined()
  })

  test('an unknown route lands on Show, not on an error', () => {
    // A booth screen that has been sitting on a stale bookmark should come up
    // usable rather than showing a 404.
    at('/nonsense')
    expect(screen.getByRole('link', { name: 'Show' }).getAttribute('aria-current')).toBe('page')
  })
})

describe('vocabulary', () => {
  test('the UI uses plain words, not internals', () => {
    at('/show')
    const body = document.body.textContent ?? ''
    // docs/UI_TEXT.md bans these from user-facing text. They are all correct
    // internally; none of them is a volunteer's problem.
    for (const banned of ['SceneDoc', 'scene graph', 'blake3', 'premultiplied', 'epoch']) {
      expect(body).not.toContain(banned)
    }
  })
})
