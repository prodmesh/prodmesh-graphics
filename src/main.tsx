import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { BrowserRouter } from 'react-router-dom'

// Bundled, never from a CDN. Booth machines have no internet, and the no-CDN
// rule is a family-wide invariant.
import '@fontsource-variable/ibm-plex-sans'
import '@fontsource/ibm-plex-mono'
import './styles/index.css'

import { App } from './App'

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <BrowserRouter>
      <App />
    </BrowserRouter>
  </StrictMode>,
)
