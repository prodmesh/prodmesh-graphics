import { useCallback, useEffect, useState } from 'react'
import { NavLink, Outlet, useLocation } from 'react-router-dom'
import logo from '../assets/prodmesh-gfx-lockup.png'
import { invalidateAll, useQuery } from '../lib/useQuery'
import { useTopic } from '../lib/stream'
import { keys, topics } from '../lib/keys'
import { confirmLeave } from '../lib/dirty'
import { StatusIndicator, TelemetryChip } from '../components/console'
import { SignInBar } from '../components/SignInBar'
import {
  AUTH_CHANGED,
  AUTH_REQUIRED,
  IdentityContext,
  ensureStation,
  fetchMe,
  signOut,
  type Denial,
  type Me,
} from '../lib/identity'
import type { LayersSnapshot, NodeSummary } from '../modes/show/types'

/**
 * The console shell (ADR 0020): a fixed header with the mode tabs, a flexing
 * centre the mode fills with panels, and a fixed footer readout. Fixed chrome,
 * flexing centre — nothing floats, nothing overlays.
 *
 * The header's right cluster is real state, never decoration: the node's
 * reachability as the node link reports it, and the output's measured frame
 * rate with its trailing zeros kept, because they are precision. When there is
 * no node there is no chip — do not invent a metric to fill a card.
 *
 * The shell also owns identity (ADR 0025). It registers this browser as a
 * station once, holds the one copy of "who is signed in and what may they do"
 * that the rest of the tree reads through `IdentityContext`, and opens the
 * sign-in band when something is refused. Nothing else fetches `/api/auth/me`.
 */

const MODES = [
  { to: '/configure', label: 'Configure' },
  { to: '/design', label: 'Design' },
  { to: '/edit', label: 'Edit' },
  { to: '/show', label: 'Show' },
] as const

export function AppShell() {
  const { data: version } = useQuery<{ server: string; milestone: string }>(keys.version())
  const { data: nodes } = useQuery<{ nodes: NodeSummary[] }>(keys.nodes())
  const location = useLocation()

  const [me, setMe] = useState<Me | null>(null)
  const [denial, setDenial] = useState<Denial | null>(null)
  const [signingIn, setSigningIn] = useState(false)

  const refreshIdentity = useCallback(() => {
    void fetchMe().then(setMe)
  }, [])

  useEffect(() => {
    void ensureStation().then(refreshIdentity)
  }, [refreshIdentity])

  useEffect(() => {
    // Signing in changes what every read is allowed to return, so the cache is
    // answers to questions asked as somebody else.
    const changed = () => {
      invalidateAll()
      refreshIdentity()
    }
    const required = (e: Event) => {
      setDenial((e as CustomEvent<Denial>).detail ?? {})
      setSigningIn(true)
    }
    window.addEventListener(AUTH_CHANGED, changed)
    window.addEventListener(AUTH_REQUIRED, required)
    return () => {
      window.removeEventListener(AUTH_CHANGED, changed)
      window.removeEventListener(AUTH_REQUIRED, required)
    }
  }, [refreshIdentity])

  const node = nodes?.nodes?.[0]
  const outputId = node?.state?.outputs?.[0]?.id
  const layers = useTopic<LayersSnapshot>(outputId ? topics.outputLayers(outputId) : null)

  const onAir = (layers?.layers ?? []).some((l) => l.occupant)
  const mode = MODES.find((m) => location.pathname.startsWith(m.to))?.label ?? 'Show'
  // An open server has nobody to sign in as, and offering it would be a door
  // onto a wall. The footer says the server is open instead.
  const signInOffered = me != null && !me.unconfigured

  return (
    <IdentityContext.Provider value={me}>
      <div className="console">
        <header className="console__header">
          <div className="c-brand">
            {/* The lockup carries the name, so there is no text beside it to
                disagree with. Bundled by Vite rather than fetched — no CDNs,
                ever, and a booth machine has no internet to fetch from. */}
            <img className="c-brand__logo" src={logo} alt="ProdMesh GFX" />
            <span className="c-brand__version">
              {version ? `v${version.server} · ${version.milestone}` : 'connecting'}
            </span>
          </div>

          <nav className="c-modetabs" aria-label="Mode">
            {MODES.map(({ to, label }) => (
              <NavLink
                key={to}
                to={to}
                // Unpublished work blocks the tab until the user says go —
                // switching modes unmounts the editor, and losing an hour of
                // design to a mis-click is not a lesson anyone needs twice.
                onClick={(e) => {
                  if (!confirmLeave()) e.preventDefault()
                }}
                className={({ isActive }) =>
                  isActive ? 'c-modetabs__tab c-modetabs__tab--active' : 'c-modetabs__tab'
                }
              >
                {label}
              </NavLink>
            ))}
          </nav>

          <div className="console__header-status">
            {/* While the band is open it IS the control; two doors marked the
                same thing is one more thing to read under time pressure. */}
            {signInOffered && !signingIn ? (
              <div className="c-identity">
                {me?.user ? (
                  <>
                    <span className="c-identity__who">{me.user.username.toUpperCase()}</span>
                    <button
                      className="c-identity__link"
                      onClick={() => {
                        void signOut()
                        setSigningIn(false)
                      }}
                    >
                      Sign out
                    </button>
                  </>
                ) : (
                  <button
                    className="c-identity__link"
                    onClick={() => {
                      setDenial(null)
                      setSigningIn(true)
                    }}
                  >
                    Sign in
                  </button>
                )}
              </div>
            ) : null}
            {layers?.fps != null ? (
              <TelemetryChip engine={node?.name ?? 'node'} value={`${layers.fps.toFixed(2)} fps`} />
            ) : null}
            {node ? (
              <StatusIndicator
                tone={node.reachable ? 'ready' : 'muted'}
                label={node.reachable ? `${node.name} online` : `${node.name} offline`}
              />
            ) : (
              <StatusIndicator tone="muted" label="No node" />
            )}
            <StatusIndicator tone={onAir ? 'air' : 'muted'} label={onAir ? 'On air' : 'Nothing on air'} />
          </div>
        </header>

        {signingIn && signInOffered ? (
          <SignInBar
            me={me}
            denial={denial}
            onDone={() => {
              setSigningIn(false)
              setDenial(null)
            }}
            onDismiss={() => {
              setSigningIn(false)
              setDenial(null)
            }}
          />
        ) : null}

        <main className="console__main">
          <Outlet />
        </main>

        <footer className="console__footer">
          <div className="console__footer-left">
            <span className="console__footer-mode">{mode.toUpperCase()} MODE</span>
            {me?.unconfigured ? (
              // Said out loud. An open server that never mentions it is a quiet
              // hole; a booth appliance locked behind a password nobody wrote
              // down is an outage. This is the third option (ADR 0013).
              <span className="console__footer-hint" role="status">
                No users yet — anyone on this network can operate this server. Add a user in Configure
                to turn permissions on.
              </span>
            ) : me && !me.user ? (
              // Read-only is a state, not a failure, and an operator who does not
              // know they are in it reads every refusal as the system being
              // broken.
              <span className="console__footer-hint" role="status">
                Signed out — this station can watch, not change.
              </span>
            ) : null}
          </div>
          <span className="console__footer-readout">
            {node
              ? `${node.name}: ${node.reachable ? 'linked' : 'offline'}${
                  layers?.late_frames != null ? `  ·  late frames: ${layers.late_frames}` : ''
                }`
              : 'No playout node has connected yet.'}
          </span>
        </footer>
      </div>
    </IdentityContext.Provider>
  )
}
