/**
 * One `upgrade` listener per HTTP server, routing WebSocket handshakes by path.
 *
 * `new WebSocketServer({ server, path })` cannot be used twice on one listener:
 * each instance installs its own `upgrade` handler, and in ws 8 a handler whose
 * path does not match ABORTS the handshake with a 400 rather than leaving it
 * for the next one. The second socket on the server — the browser's cue socket
 * beside PMG Link — would have broken whichever was attached first.
 */

const tables = new WeakMap()

/** Serve WebSocket upgrades on `path` with `wss`, a `noServer` WebSocketServer. */
export function route(server, path, wss) {
  let table = tables.get(server)
  if (!table) {
    table = new Map()
    tables.set(server, table)
    server.on('upgrade', (req, socket, head) => {
      const { pathname } = new URL(req.url ?? '/', 'http://localhost')
      const target = table.get(pathname)
      if (!target) {
        socket.write('HTTP/1.1 404 Not Found\r\nConnection: close\r\n\r\n')
        socket.destroy()
        return
      }
      target.handleUpgrade(req, socket, head, (ws) => target.emit('connection', ws, req))
    })
  }
  table.set(path, wss)
}
