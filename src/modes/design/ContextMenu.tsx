/**
 * A menu at the pointer, shared by everything in Design that has a right-click.
 *
 * Lives in its own module rather than inside `Design.tsx` because the timeline
 * panel needs it too, and `TimelinePanel` importing from `Design` would be a
 * cycle. Closes on any press elsewhere, Escape, or leaving the window.
 */

import { useEffect } from 'react'

export type MenuItem =
  | { label: string; disabled?: boolean; danger?: boolean; onPick: () => void }
  | 'rule'

export function ContextMenu({
  x,
  y,
  items,
  onClose,
}: {
  x: number
  y: number
  items: MenuItem[]
  onClose: () => void
}) {
  useEffect(() => {
    const close = () => onClose()
    const key = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose()
    }
    // Bubble phase, so a press inside the menu (which stops propagation)
    // reaches its item before anything closes.
    window.addEventListener('pointerdown', close)
    window.addEventListener('keydown', key)
    window.addEventListener('blur', close)
    return () => {
      window.removeEventListener('pointerdown', close)
      window.removeEventListener('keydown', key)
      window.removeEventListener('blur', close)
    }
  }, [onClose])
  const style: React.CSSProperties = {
    left: Math.min(x, window.innerWidth - 190),
    top: Math.min(y, window.innerHeight - items.length * 28 - 20),
  }
  return (
    <div
      className="ctx"
      role="menu"
      style={style}
      onPointerDown={(e) => e.stopPropagation()}
      onContextMenu={(e) => e.preventDefault()}
    >
      {items.map((it, i) =>
        it === 'rule' ? (
          <div key={`rule-${i}`} className="ctx__rule" role="separator" />
        ) : (
          <button
            key={it.label}
            type="button"
            role="menuitem"
            disabled={it.disabled}
            className={it.danger ? 'ctx__item ctx__item--danger' : 'ctx__item'}
            onClick={() => {
              onClose()
              it.onPick()
            }}
          >
            {it.label}
          </button>
        ),
      )}
    </div>
  )
}
