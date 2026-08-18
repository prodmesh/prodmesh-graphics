import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { BrowserRouter } from 'react-router-dom'

// Bundled, never from a CDN. Booth machines have no internet, and the no-CDN
// rule is a family-wide invariant. The design system named these three from
// Google Fonts; they ship in the bundle instead (ADR 0020).
import '@fontsource-variable/inter'
import '@fontsource-variable/spline-sans-mono'
import '@fontsource/instrument-serif'
import './styles/index.css'

import { App } from './App'

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <BrowserRouter>
      <App />
    </BrowserRouter>
  </StrictMode>,
)
