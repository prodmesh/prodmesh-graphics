import { Navigate, Route, Routes } from 'react-router-dom'
import { AppShell } from './layout/AppShell'
import { Configure } from './modes/configure/Configure'
import { Design } from './modes/design/Design'
import { Edit } from './modes/edit/Edit'
import { Show } from './modes/show/Show'

/**
 * Four modes, separated by mindset rather than by permission (ADR 0015).
 *
 * Show is deliberately reachable without passing through anything destructive,
 * and there is no route from Show into Design — an operator under time pressure
 * should not be one mis-tap from the template editor.
 */
export function App() {
  return (
    <Routes>
      <Route element={<AppShell />}>
        <Route path="/configure/*" element={<Configure />} />
        <Route path="/design/*" element={<Design />} />
        <Route path="/edit/*" element={<Edit />} />
        <Route path="/show/*" element={<Show />} />
        <Route path="*" element={<Navigate to="/show" replace />} />
      </Route>
    </Routes>
  )
}
