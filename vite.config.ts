import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

// The control server owns /api, /cas, and /link. In dev, Vite proxies to it so
// the browser sees one origin — which matters more than usual here: a LAN
// appliance has no TLS and therefore no HTTP/2, so the six-connections-per-origin
// limit is real (ADR 0008). Splitting origins in dev would hide that.
export default defineConfig({
  plugins: [react()],
  server: {
    port: 5173,
    proxy: {
      // Before /api, which would otherwise claim it without `ws`.
      '/api/cue/socket': { target: 'ws://localhost:8519', ws: true },
      '/api': { target: 'http://localhost:8519', changeOrigin: true },
      '/cas': { target: 'http://localhost:8519', changeOrigin: true },
      '/link': { target: 'ws://localhost:8519', ws: true },
    },
  },
  build: { outDir: 'dist', sourcemap: true },
})
