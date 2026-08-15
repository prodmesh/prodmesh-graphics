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
  test.each([
    ['/configure', 'Configure'],
    ['/design', 'Design'],
    ['/edit', 'Edit'],
    ['/show', 'Show'],
  ])('%s renders %s', (path, heading) => {
    at(path)
    expect(screen.getByRole('heading', { level: 1, name: heading })).toBeDefined()
  })

  test('an unknown route lands on Show, not on an error', () => {
    // A booth screen that has been sitting on a stale bookmark should come up
    // usable rather than showing a 404.
    at('/nonsense')
    expect(screen.getByRole('heading', { level: 1, name: 'Show' })).toBeDefined()
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
