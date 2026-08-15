import { NavLink, Outlet } from 'react-router-dom'
import { Cog, PenTool, ListOrdered, MonitorPlay } from 'lucide-react'
import { useQuery } from '../lib/useQuery'
import { keys } from '../lib/keys'

/**
 * The shell: a left sidebar over the four modes, collapsing to an icon rail on
 * narrow screens. ProdMesh-Control's shape, so the two apps feel like one
 * product.
 */

const MODES = [
  { to: '/configure', label: 'Configure', Icon: Cog },
  { to: '/design', label: 'Design', Icon: PenTool },
  { to: '/edit', label: 'Edit', Icon: ListOrdered },
  { to: '/show', label: 'Show', Icon: MonitorPlay },
] as const

export function AppShell() {
  const { data } = useQuery<{ server: string; milestone: string }>(keys.version())

  return (
    <div className="shell">
      <nav className="shell__sidebar">
        <div className="shell__brand">
          ProdMesh <span>Graphics</span>
        </div>
        <div className="shell__nav">
          {MODES.map(({ to, label, Icon }) => (
            <NavLink
              key={to}
              to={to}
              className={({ isActive }) =>
                isActive ? 'shell__link shell__link--active' : 'shell__link'
              }
            >
              <Icon size={17} aria-hidden />
              <span>{label}</span>
            </NavLink>
          ))}
        </div>
        <div className="shell__hint">
          {data ? `v${data.server} · ${data.milestone}` : 'connecting'}
        </div>
      </nav>
      <main className="shell__main">
        <Outlet />
      </main>
    </div>
  )
}
