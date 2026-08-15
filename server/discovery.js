/**
 * DNS-SD advertisement, so a node can find this server without being told an
 * address.
 *
 * # mDNS is an aid, never an authority
 *
 * Multicast is genuinely unreliable across VLANs and behind switches with poor
 * IGMP snooping. **Manual host entry must work everywhere discovery appears**,
 * and an operator who can type an address must never be blocked by a broken
 * multicast domain (`docs/specs/pmg-link.md` §1). This module failing is
 * therefore a warning, not a startup error.
 */

import { Bonjour } from 'bonjour-service'

export const SERVICE_TYPE = 'pmg-control'

let bonjour = null
let service = null

export function advertise({ port, serverId, name = 'ProdMesh Graphics', protoVer = 1 } = {}) {
  try {
    // The error callback is NOT optional here. bonjour-service's default is
    // `function (err) { throw err }` from inside a socket callback, which is an
    // uncaught exception on the event loop — so a multicast problem, an
    // interface going away, or another responder holding 5353 would take down
    // the CONTROL SERVER. A convenience feature must not be able to do that.
    bonjour = new Bonjour(undefined, (err) => {
      console.warn('discovery: mDNS error (advertisement only, the server is fine) —', err?.message ?? err)
    })
    service = bonjour.publish({
      name: `${name} (${String(serverId).slice(0, 8)})`,
      type: SERVICE_TYPE,
      protocol: 'tcp',
      port,
      txt: { ver: String(protoVer), id: String(serverId), name },
    })
    return true
  } catch (e) {
    // A server that cannot advertise is still a server. Nodes reach it by
    // address, which is the path that has to work anyway.
    console.warn('discovery: could not advertise over mDNS —', e.message)
    return false
  }
}

export function stop() {
  try {
    service?.stop?.()
    bonjour?.destroy?.()
  } catch { /* shutting down */ }
  service = null
  bonjour = null
}
