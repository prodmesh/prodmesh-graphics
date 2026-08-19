import { NavLink, Outlet, useLocation } from 'react-router-dom'
import { Microchip } from 'lucide-react'
import { useQuery } from '../lib/useQuery'
import { useTopic } from '../lib/stream'
import { keys, topics } from '../lib/keys'
import { confirmLeave } from '../lib/dirty'
import { StatusIndicator, TelemetryChip } from '../components/console'
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
 */

const MODES = [
  { to: '/configure', label: 'Configure' },
  { to: '/design', label: 'Design' },
  { to: '/edit', label: 'Edit' },
  { to: '/show', label: 'Show' },
] as const

export function AppShell() {
  const { data: version } = useQuery<{ server: string; milestone: string }>(keys.version())
  const { data: me } = useQuery<{ unconfigured: boolean }>('/api/auth/me')
  const { data: nodes } = useQuery<{ nodes: NodeSummary[] }>(keys.nodes())
  const location = useLocation()

  const node = nodes?.nodes?.[0]
  const outputId = node?.state?.outputs?.[0]?.id
  const layers = useTopic<LayersSnapshot>(outputId ? topics.outputLayers(outputId) : null)

  const onAir = (layers?.layers ?? []).some((l) => l.occupant)
  const mode = MODES.find((m) => location.pathname.startsWith(m.to))?.label ?? 'Show'

  return (
    <div className="console">
      <header className="console__header">
        <div className="c-brand">
          <span className="c-brand__mark">
            <Microchip size={14} aria-hidden />
          </span>
          <span className="c-brand__name">
            ProdMesh <span>Graphics</span>
          </span>
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
  )
}
