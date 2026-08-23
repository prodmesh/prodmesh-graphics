/**
 * The console component library (ADR 0020).
 *
 * Transcribed from the ProdMesh GFX design system. Presentation only: nothing
 * in this file fetches, subscribes, or knows a route. Styles live in
 * `src/styles/console.css`, tokens in `src/styles/tokens.css`.
 *
 * The vocabulary these components render is normative (docs/UI_TEXT.md):
 * state runs are ALL CAPS with 1px tracking, buttons are sentence case, and
 * measured numbers keep their trailing zeros because they are precision.
 */

import type { ReactNode } from 'react'

export type Tone = 'air' | 'armed' | 'ready' | 'staged' | 'muted'

// ---------------------------------------------------------------------------
// Status

export function StatusDot({ tone = 'ready', size = 8 }: { tone?: Tone; size?: number }) {
  return <span className={`c-dot c-dot--${tone}`} style={{ width: size, height: size }} />
}

export function StatusPill({
  children,
  tone = 'staged',
  variant = 'solid',
  size = 9,
}: {
  children: ReactNode
  tone?: Tone
  variant?: 'solid' | 'subtle'
  size?: 9 | 10
}) {
  // Only air and armed exist as solid fills; everything calmer is subtle.
  const solid = variant === 'solid' && (tone === 'air' || tone === 'armed')
  const cls = [
    'c-pill',
    solid ? `c-pill--solid-${tone}` : `c-pill--subtle-${tone}`,
    size >= 10 ? 'c-pill--lg' : '',
    String(children) === 'ON AIR' ? 'c-pill--caps' : '',
  ]
    .filter(Boolean)
    .join(' ')
  return <span className={cls}>{children}</span>
}

export function Tag({
  children,
  tone = 'quiet',
  pill = false,
  size = 11,
}: {
  children: ReactNode
  tone?: 'quiet' | 'raised' | 'armed'
  pill?: boolean
  size?: 10 | 11
}) {
  const cls = [
    'c-tag',
    tone !== 'quiet' ? `c-tag--${tone}` : '',
    pill ? 'c-tag--pill' : '',
    size === 10 ? 'c-tag--sm' : '',
  ]
    .filter(Boolean)
    .join(' ')
  return <span className={cls}>{children}</span>
}

export function StatusIndicator({ label, tone = 'ready' }: { label: string; tone?: Tone }) {
  return (
    <div className="c-status">
      <StatusDot tone={tone} />
      <span className="c-status__label">{label}</span>
    </div>
  )
}

// Real numbers only. Do not invent a metric to fill this chip — it exists for
// values the system actually measured, trailing zeros intact.
export function TelemetryChip({
  engine,
  value,
  healthy = true,
}: {
  engine: string
  value: string
  healthy?: boolean
}) {
  return (
    <div className="c-telemetry">
      <span className={healthy ? 'c-telemetry__engine' : 'c-telemetry__engine c-telemetry__engine--unhealthy'}>
        {engine}
      </span>
      <span className="c-telemetry__divider" />
      <span className="c-telemetry__value">{value}</span>
    </div>
  )
}

// ---------------------------------------------------------------------------
// Controls

export function Button({
  children,
  variant = 'primary',
  take = false,
  block = false,
  disabled = false,
  onClick,
  title,
}: {
  children: ReactNode
  variant?: 'primary' | 'secondary' | 'quiet'
  /** The commit button. Carries the one drop shadow in the product. */
  take?: boolean
  block?: boolean
  disabled?: boolean
  onClick?: () => void
  title?: string
}) {
  const cls = [
    'c-btn',
    `c-btn--${variant}`,
    take ? 'c-btn--take' : '',
    block ? 'c-btn--block' : '',
  ]
    .filter(Boolean)
    .join(' ')
  return (
    <button type="button" className={cls} disabled={disabled} onClick={onClick} title={title}>
      {children}
    </button>
  )
}

export function Keycap({ children }: { children: ReactNode }) {
  return <span className="c-keycap">{children}</span>
}

export function ShortcutRow({ label, keys }: { label: string; keys: string[] }) {
  return (
    <div className="c-shortcut">
      <span className="c-shortcut__label">{label}</span>
      <span className="c-shortcut__keys">
        {keys.map((k) => (
          <Keycap key={k}>{k}</Keycap>
        ))}
      </span>
    </div>
  )
}

/**
 * A labelled value box. Pass `children` for an editable control (input,
 * select), or `value` for a read-only fact.
 */
export function FormGroup({
  label,
  value,
  mono = false,
  children,
}: {
  label: string
  value?: string
  mono?: boolean
  children?: ReactNode
}) {
  // A real <label>, wrapping its control rather than pointing at one by id.
  // It was a <span>, which meant every field in the product was nameless to a
  // screen reader and to anything driving the UI by label — and clicking the
  // caption did nothing. Wrapping needs no id plumbing and cannot come adrift
  // from the control it names.
  return (
    <label className="c-formgroup">
      <span className="c-formgroup__label">{label}</span>
      <div className="c-formgroup__box">
        {children ?? (
          <span className={mono ? 'c-formgroup__value c-formgroup__value--mono' : 'c-formgroup__value'}>
            {value}
          </span>
        )}
      </div>
    </label>
  )
}

export function KeyValueRow({
  label,
  value,
  tone = 'armed',
}: {
  label: string
  value: string
  tone?: 'armed' | 'plain'
}) {
  return (
    <div className="c-kv">
      <span className="c-kv__label">{label}</span>
      <span className={tone === 'armed' ? 'c-kv__value c-kv__value--armed' : 'c-kv__value'}>{value}</span>
    </div>
  )
}

// ---------------------------------------------------------------------------
// Grouping and logs

export function GroupHeader({
  label,
  trailing,
  tone = 'muted',
}: {
  label: string
  trailing?: string
  tone?: 'muted' | 'armed'
}) {
  return (
    <div className={tone === 'armed' ? 'c-grouphead c-grouphead--armed' : 'c-grouphead'}>
      <span className="c-grouphead__label">{label}</span>
      {trailing ? <span className="c-grouphead__trailing">{trailing}</span> : null}
    </div>
  )
}

export function LogRow({ entry, result }: { entry: string; result: string }) {
  const tone =
    result === 'LIVE' ? 'c-logrow__result--live' : result === 'STAGE' ? 'c-logrow__result--stage' : ''
  return (
    <div className="c-logrow">
      <span className="c-logrow__entry">{entry}</span>
      <span className={`c-logrow__result ${tone}`.trim()}>{result}</span>
    </div>
  )
}

export function ProgressBar({
  value,
  total,
  tone = 'armed',
}: {
  value: number
  total: number
  tone?: 'armed' | 'air'
}) {
  const pct = Math.max(0, Math.min(1, total ? value / total : 0))
  return (
    <div className="c-progress">
      <div
        className={tone === 'air' ? 'c-progress__fill c-progress__fill--air' : 'c-progress__fill'}
        style={{ width: `${pct * 100}%` }}
      />
    </div>
  )
}

// ---------------------------------------------------------------------------
// Panels

export function Panel({
  width,
  children,
  label,
}: {
  width?: number | string
  children: ReactNode
  label?: string
}) {
  return (
    <section className="c-panel" style={width ? { width } : { flexGrow: 1, flexShrink: 1 }} aria-label={label}>
      {children}
    </section>
  )
}

export function PanelHeader({
  title,
  icon,
  trailing,
}: {
  title: string
  icon?: ReactNode
  trailing?: ReactNode
}) {
  return (
    <header className="c-panel__header">
      <div className="c-panel__title">
        {icon}
        <span>{title}</span>
      </div>
      {trailing}
    </header>
  )
}

/** The scrolling interior of a Panel. `pad16` for inspectors, default for rails. */
export function PanelBody({ children, pad16 = false }: { children: ReactNode; pad16?: boolean }) {
  return <div className={pad16 ? 'c-panel__body c-panel__body--pad16' : 'c-panel__body'}>{children}</div>
}

// ---------------------------------------------------------------------------
// Playout

export type RowState = 'idle' | 'armed' | 'air'

/**
 * A rundown row. `state` is one of the two marks ADR 0017 requires stay
 * separate: `armed` renders the cursor, `air` renders what the node reports,
 * and a row can only show one — the caller decides which wins when both hold
 * (air does; the cursor is also shown by the list position).
 */
export function RundownItem({
  id,
  title,
  subtitle,
  state = 'idle',
  pill,
  dot,
  disabled = false,
  onClick,
}: {
  id?: string
  title: string
  subtitle?: string
  state?: RowState
  pill?: { label: string; tone: Tone; variant?: 'solid' | 'subtle' }
  dot?: Tone
  disabled?: boolean
  onClick?: () => void
}) {
  const cls = ['c-rundownitem', state !== 'idle' ? `c-rundownitem--${state}` : '']
    .filter(Boolean)
    .join(' ')
  return (
    <button type="button" className={cls} disabled={disabled} onClick={onClick}>
      {id ? <span className="c-rundownitem__id">{id}</span> : null}
      <span className="c-rundownitem__text">
        <span className="c-rundownitem__title">{title}</span>
        {subtitle ? <span className="c-rundownitem__subtitle">{subtitle}</span> : null}
      </span>
      {pill ? (
        <StatusPill tone={pill.tone} variant={pill.variant ?? 'solid'}>
          {pill.label}
        </StatusPill>
      ) : dot ? (
        <StatusDot tone={dot} />
      ) : null}
    </button>
  )
}

export function SectionCard({
  label,
  state,
  active = false,
  disabled = false,
  onClick,
}: {
  label: string
  state: string
  active?: boolean
  disabled?: boolean
  onClick?: () => void
}) {
  return (
    <button
      type="button"
      className={active ? 'c-sectioncard c-sectioncard--active' : 'c-sectioncard'}
      disabled={disabled}
      onClick={onClick}
    >
      <span className="c-sectioncard__label">{label}</span>
      <span className="c-sectioncard__state">{state}</span>
    </button>
  )
}

export type TileState = 'upcoming' | 'past' | 'armed' | 'air'

/** A slide tile in the Follow grid. Same two marks as RundownItem. */
export function SlideTile({
  num,
  line1,
  line2,
  state = 'upcoming',
  pill,
  disabled = false,
  onClick,
}: {
  num: string
  line1: string
  line2?: string
  state?: TileState
  pill?: { label: string; tone: Tone }
  disabled?: boolean
  onClick?: () => void
}) {
  const cls = ['c-tile', state !== 'upcoming' ? `c-tile--${state}` : ''].filter(Boolean).join(' ')
  return (
    <button type="button" className={cls} disabled={disabled} onClick={onClick}>
      <span className="c-tile__head">
        <span className="c-tile__num">{num}</span>
        {pill ? <StatusPill tone={pill.tone}>{pill.label}</StatusPill> : null}
      </span>
      <span className="c-tile__line1">{line1}</span>
      {line2 ? <span className="c-tile__line2">{line2}</span> : null}
    </button>
  )
}

export function LayerCard({
  layer,
  state,
  tone = 'ready',
  content,
  action,
  onAction,
  actionDisabled = false,
  children,
}: {
  layer: string
  state: string
  tone?: Tone
  content: string
  action?: string
  onAction?: () => void
  actionDisabled?: boolean
  /** Extra controls between the content line and the action — cue chips. */
  children?: ReactNode
}) {
  const air = tone === 'air'
  const stateCls =
    tone === 'air'
      ? 'c-layercard__state c-layercard__state--air'
      : tone === 'muted' || tone === 'staged'
        ? 'c-layercard__state c-layercard__state--muted'
        : 'c-layercard__state'
  return (
    <div className={air ? 'c-layercard c-layercard--air' : 'c-layercard'}>
      <div className="c-layercard__head">
        <span className="c-layercard__name">{layer}</span>
        <span className={stateCls}>{state}</span>
      </div>
      <span className="c-layercard__content">{content}</span>
      {children}
      {action ? (
        <Button variant={air ? 'primary' : 'quiet'} block onClick={onAction} disabled={actionDisabled}>
          {action}
        </Button>
      ) : null}
    </div>
  )
}

// ---------------------------------------------------------------------------
// Navigation

export function NavItem({
  title,
  meta,
  icon,
  active = false,
  onClick,
}: {
  title: string
  meta?: string
  icon?: ReactNode
  active?: boolean
  onClick?: () => void
}) {
  return (
    <button
      type="button"
      className={active ? 'c-navitem c-navitem--active' : 'c-navitem'}
      onClick={onClick}
    >
      {icon}
      <span className="c-navitem__text">
        <span className="c-navitem__title">{title}</span>
        {meta ? <span className="c-navitem__meta">{meta}</span> : null}
      </span>
    </button>
  )
}

export function DataTable({
  columns,
  rows,
}: {
  columns: { label: string; width?: string }[]
  rows: ReactNode[][]
}) {
  const grid = columns.map((c) => c.width ?? '1fr').join(' ')
  return (
    <div className="c-table">
      <div className="c-table__head" style={{ gridTemplateColumns: grid }}>
        {columns.map((c) => (
          <span key={c.label} className="c-table__col">
            {c.label}
          </span>
        ))}
      </div>
      {rows.map((r, i) => (
        <div key={i} className="c-table__row" style={{ gridTemplateColumns: grid }}>
          {r.map((cell, j) => (
            <span key={j} className={j === 0 ? 'c-table__cell c-table__cell--first' : 'c-table__cell'}>
              {cell}
            </span>
          ))}
        </div>
      ))}
    </div>
  )
}

export function NodeCard({
  name,
  role,
  metrics,
  status,
  healthy = true,
}: {
  name: string
  role?: string
  metrics: { label: string; value: string; accent?: boolean }[]
  status?: string
  healthy?: boolean
}) {
  return (
    <div className="c-nodecard">
      <div className="c-nodecard__head">
        <span className="c-nodecard__name">{name}</span>
        {role ? <Tag size={10}>{role}</Tag> : null}
      </div>
      <div className="c-nodecard__metrics">
        {metrics.map((m) => (
          <div key={m.label} className="c-nodecard__metric">
            <span className="c-nodecard__metric-label">{m.label}</span>
            <span
              className={
                m.accent ? 'c-nodecard__metric-value c-nodecard__metric-value--accent' : 'c-nodecard__metric-value'
              }
            >
              {m.value}
            </span>
          </div>
        ))}
      </div>
      {status ? (
        <span className={healthy ? 'c-nodecard__status' : 'c-nodecard__status c-nodecard__status--unhealthy'}>
          {status}
        </span>
      ) : null}
    </div>
  )
}

// ---------------------------------------------------------------------------
// Broadcast

/**
 * The program well: header row (label, ON AIR pill, output caption) over a
 * dark canvas. The well is darker than everything else so real output reads
 * as the brightest thing on screen. Children are the frame source — a
 * <canvas> fed by WebCodecs in Show, the WASM canvas in Design.
 *
 * Never substitute a grey box for a render: when there is no frame, say so
 * through `empty`, in words that name which thing is absent — "no preview"
 * and "nothing on air" need different responses from an operator.
 */
export function ProgramCanvas({
  label,
  onAir = false,
  output,
  empty,
  children,
}: {
  label: string
  onAir?: boolean
  output?: string
  /** The sentence shown when there is no frame to show. */
  empty?: string | null
  children?: ReactNode
}) {
  return (
    <div className="c-program">
      <div className="c-program__head">
        <span className="c-program__label">
          {label}
          {onAir ? <StatusPill tone="air">ON AIR</StatusPill> : null}
        </span>
        {output ? <span className="c-program__output">{output}</span> : null}
      </div>
      <div className="c-program__well">
        {children}
        {empty ? <div className="c-program__empty">{empty}</div> : null}
      </div>
    </div>
  )
}
