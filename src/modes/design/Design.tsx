/**
 * Design — for the person who makes it look right.
 *
 * The console layout (ADR 0020): template browser in the left rail, the canvas
 * in the centre workspace, template facts and publishing in the right
 * inspector. Show has no route here, on purpose (ADR 0015).
 *
 * # The canvas is the output, not a picture of it
 *
 * It renders with `pmg-render` compiled to WASM — the same code the playout
 * node runs. That is the whole premise of ADR 0001, and it is what makes the
 * preview trustworthy rather than indicative. The `template_rev` this mode
 * computes when it publishes is byte-identical to the one `pmgctl` computes for
 * the same document.
 *
 * # It is drawn on a checkerboard, in CSS
 *
 * The output is a **key signal**, so the canvas is cleared to transparent and
 * the checkerboard lives behind it in a stylesheet. Baking a background into
 * the pixels would hide exactly the thing a designer needs to see — what will
 * actually key over live video.
 *
 * # Fonts come from the CAS
 *
 * There is no system font database. A template naming a system font would
 * render one way here and another on a booth Mac, silently. Fonts are assets
 * carried by content hash, and this mode fetches them the same way a node does.
 *
 * Every operation is reachable without a drag: jsdom has no layout, so a
 * simulated drag certifies nothing, and the button-and-keyboard paths are what
 * the tests can actually cover.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { AlertTriangle, Clapperboard, FileUp, FolderOpen, Image as ImageIcon, Plus, Redo2, Scissors, Trash2, Type, Undo2, Upload } from 'lucide-react'
import { useQuery, invalidate } from '../../lib/useQuery'
import { keys } from '../../lib/keys'
import { ApiError, get, post } from '../../lib/api'
import { confirmLeave, setDirty } from '../../lib/dirty'
import {
  Button,
  FormGroup,
  GroupHeader,
  KeyValueRow,
  NavItem,
  Panel,
  PanelBody,
  PanelHeader,
  StatusPill,
  Tag,
} from '../../components/console'
import {
  blankDoc,
  compileTemplate,
  fontFacts,
  migrateDoc,
  useDesignCanvas,
  type DesignSession,
  type DocTimeline,
  type FaceInfo,
  type Handles,
  type NodeFacts,
} from './useDesign'
import { newId } from '../../lib/ids'
import { MediaBrowser } from './MediaBrowser'
import { ThemeManager } from './ThemeManager'
import { useDocHistory } from './useHistory'
import { LayerTree } from './LayerTree'
import { TimelinePanel } from './TimelinePanel'
import { CanvasStage } from './CanvasStage'
import type { Item } from '../show/types'

type TemplateSummary = { id: string; name: string; head_rev: string | null }

/** A theme as the server lists it (ADR 0021). The tokens JSON is a listing
 *  copy; the canonical bytes live in the CAS under `head_hash`. */
type ThemeToken = {
  color?: { rgba: [number, number, number, number] }
  font?: { family: string; weight: number }
}
type ThemeRow = {
  id: string
  name: string
  head_hash: string
  tokens: Record<string, ThemeToken>
  updated_at: number
}
/** What the token editors need, in one place. */
type ActiveTheme = {
  hash: string
  name: string
  json: string
  colorTokens: string[]
  fontTokens: string[]
}

const themeToActive = (t: ThemeRow): ActiveTheme => ({
  hash: t.head_hash,
  name: t.name,
  json: JSON.stringify({ name: t.name, tokens: t.tokens }),
  colorTokens: Object.keys(t.tokens).filter((k) => 'color' in t.tokens[k]!),
  fontTokens: Object.keys(t.tokens).filter((k) => 'font' in t.tokens[k]!),
})

/**
 * "Follows" — which theme token a slot takes its value from. Custom releases
 * the binding and keeps the value; the value inputs disable while bound,
 * because the token is now the one place the colour comes from.
 */
function TokenSelect({
  label,
  tokens,
  value,
  onPick,
}: {
  label: string
  tokens: string[]
  value: string | null
  onPick: (t: string | null) => void
}) {
  return (
    <FormGroup label="Follows">
      <select
        value={value ?? ''}
        aria-label={label}
        onChange={(e) => onPick(e.target.value || null)}
      >
        <option value="">Custom</option>
        {tokens.map((t) => (
          <option key={t} value={t}>{t}</option>
        ))}
      </select>
    </FormGroup>
  )
}
type TemplateDetail = {
  head_rev: string | null
  head: { mirror_hash: string | null; manifest: { fields: { key: string; sample?: string }[] } } | null
}
type Asset = { hash: string; size: number; kind: string }

const FONT_KINDS = new Set(['font'])

/** Encoded 0..1 channel to a two-digit hex pair. */
const hex2 = (v: number) =>
  Math.round(Math.max(0, Math.min(1, v)) * 255)
    .toString(16)
    .padStart(2, '0')

const rgbToHex = (c: [number, number, number, number]) => `#${hex2(c[0])}${hex2(c[1])}${hex2(c[2])}`

const hexToRgb = (hex: string): [number, number, number] | null => {
  const m = /^#([0-9a-f]{6})$/i.exec(hex)
  if (!m) return null
  const n = parseInt(m[1]!, 16)
  return [((n >> 16) & 255) / 255, ((n >> 8) & 255) / 255, (n & 255) / 255]
}

/**
 * The selected node: solved geometry above, authored properties below.
 *
 * X/Y are read from the solved geometry — where the node actually is, which
 * with flexbox is an answer, not a setting. Everything editable writes back
 * through the same session the handles use, so the inspector and the canvas
 * cannot disagree about what an edit means. The colour input speaks the
 * document's encoded values directly: what the designer picks is what an Item
 * author later reads.
 */
/**
 * A style a node either has or does not — Fill, Outline, Shadow, a background
 * image. The checkbox IS the add/remove: enabling applies a sensible default
 * and reveals the controls; disabling clears the style and collapses them.
 * Replaces the add/remove button pairs, which read as actions when these are
 * really states.
 */
function OptionalSection({
  label,
  on,
  onEnable,
  onDisable,
  offNote,
  children,
}: {
  label: string
  on: boolean
  onEnable: () => void
  onDisable: () => void
  /** Shown while OFF, when off has a consequence worth stating. */
  offNote?: string
  children?: React.ReactNode
}) {
  return (
    <section className="opt">
      <label className="opt__head">
        <input
          type="checkbox"
          checked={on}
          aria-label={label}
          onChange={(e) => (e.target.checked ? onEnable() : onDisable())}
        />
        <span>{label}</span>
      </label>
      {on ? <div className="opt__body">{children}</div> : null}
      {!on && offNote ? <p className="design-console__dim">{offNote}</p> : null}
    </section>
  )
}

/**
 * The element menu, at the pointer. Force of habit for most hands — cut, copy,
 * paste, duplicate live here and on the matching Cmd keys, backed by the same
 * clipboard. Closes on any press elsewhere, Escape, or leaving the window.
 */
type MenuItem = { label: string; disabled?: boolean; danger?: boolean; onPick: () => void } | 'rule'

function ContextMenu({
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

type GradStops = [[number, number, number, number], number][]

/**
 * Angle and stops for a gradient fill. The dial is CSS's — 0° up, clockwise,
 * 180° top-to-bottom — because that is the dial designers already know. Stops
 * are 2 to 4; the ends stay put and middles can come and go.
 */
function GradientControls({
  gradient,
  onCommit,
}: {
  gradient: { angle: number; stops: GradStops }
  onCommit: (angle: number, stops: GradStops) => void
}) {
  const { angle, stops } = gradient
  const withStop = (i: number, next: [[number, number, number, number], number]) => {
    const s = stops.map((x, j) => (j === i ? next : x)) as GradStops
    onCommit(angle, s)
  }
  return (
    <>
      <FormGroup label="Angle °">
        <input
          type="number"
          step={5}
          key={`ga:${angle}`}
          defaultValue={angle}
          aria-label="Gradient angle"
          onBlur={(e) => {
            const v = Number(e.target.value)
            if (Number.isFinite(v) && v !== angle) onCommit(v, stops)
          }}
        />
      </FormGroup>
      {stops.map(([c, at], i) => (
        <div key={`${i}:${at}`} className="design-console__gradstop">
          <input
            type="color"
            value={rgbToHex(c)}
            aria-label={`Stop ${i + 1} colour`}
            onChange={(e) => {
              const rgb = hexToRgb(e.target.value)
              if (rgb) withStop(i, [[rgb[0], rgb[1], rgb[2], c[3]], at])
            }}
          />
          <input
            type="number"
            min={0}
            max={100}
            key={`gat:${at}`}
            defaultValue={Math.round(at * 100)}
            aria-label={`Stop ${i + 1} position %`}
            disabled={i === 0 || i === stops.length - 1}
            onBlur={(e) => {
              const v = Number(e.target.value)
              if (Number.isFinite(v)) withStop(i, [c, Math.max(0, Math.min(100, v)) / 100])
            }}
          />
          <span>%</span>
          {i > 0 && i < stops.length - 1 ? (
            <button
              type="button"
              className="edit-console__iconbtn"
              aria-label={`Remove stop ${i + 1}`}
              onClick={() => onCommit(angle, stops.filter((_, j) => j !== i) as GradStops)}
            >
              <Trash2 size={12} aria-hidden />
            </button>
          ) : null}
        </div>
      ))}
      {stops.length < 4 ? (
        <button
          type="button"
          className="edit-console__addbtn"
          onClick={() => {
            // A new stop lands midway through the last interval, in the
            // colour already passing through that point.
            const a = stops[stops.length - 2]!
            const b = stops[stops.length - 1]!
            const at = (a[1] + b[1]) / 2
            const mid = a[0].map((v, i) => (v + b[0][i]!) / 2) as [number, number, number, number]
            const s = [...stops.slice(0, -1), [mid, at], stops[stops.length - 1]] as GradStops
            onCommit(angle, s)
          }}
        >
          <Plus size={12} aria-hidden /> Add a stop
        </button>
      ) : null}
    </>
  )
}

function SelectedNode({
  session,
  ready,
  generation,
  selected,
  faces,
  imageAssets,
  theme,
  onEdit,
  onDelete,
}: {
  session: React.RefObject<DesignSession | null>
  ready: boolean
  /** The session's replacement counter — geometry is only new when it is. */
  generation: number
  selected: string
  /** The faces this server holds, by their files' own names — what the font
   *  picker offers. */
  faces: FaceInfo[]
  /** Image assets this server holds — what the picture picker offers. */
  imageAssets: { hash: string; size: number }[]
  /** The theme this document follows, when it follows one — what the token
   *  pickers offer (ADR 0021). */
  theme: ActiveTheme | null
  onEdit: (doc: Record<string, unknown>) => void
  /** Absent for the root: the root is the canvas, not a thing to delete. */
  onDelete?: () => void
}) {
  const [bbox, setBbox] = useState<[number, number, number, number] | null>(null)
  const [facts, setFacts] = useState<NodeFacts | null>(null)
  const [w, setW] = useState('')
  const [h, setH] = useState('')
  const [radius, setRadius] = useState('')
  const [opacity, setOpacity] = useState('')
  const [alpha, setAlpha] = useState('')
  const [expr, setExpr] = useState('')
  const [fontSize, setFontSize] = useState('')
  const [family, setFamily] = useState('')
  const [weight, setWeight] = useState('')

  useEffect(() => {
    const s = session.current
    if (!ready || !s) return
    try {
      const hd = JSON.parse(s.handles(selected)) as Handles
      setBbox(hd.bbox)
      setW(String(Math.round(hd.bbox[2])))
      setH(String(Math.round(hd.bbox[3])))
      const f = JSON.parse(s.facts(selected)) as NodeFacts
      setFacts(f)
      setRadius(String(f.radius))
      setOpacity(String(Math.round(f.opacity * 100)))
      setAlpha(String(Math.round((f.fill?.[3] ?? 1) * 100)))
      setExpr(f.text?.expr ?? '')
      setFontSize(f.text ? String(f.text.size) : '')
      setFamily(f.text?.family ?? '')
      setWeight(f.text ? String(f.text.weight) : '')
    } catch {
      setBbox(null)
      setFacts(null)
    }
  }, [session, ready, generation, selected])

  /** Run one session edit and hand the edited document up. */
  const edit = (fn: (s: DesignSession) => string) => {
    const s = session.current
    if (!s) return
    try {
      onEdit(JSON.parse(fn(s)) as Record<string, unknown>)
    } catch {
      // The node may be gone; the selection clears upstream.
    }
  }

  const commitSize = () => {
    if (!facts || !bbox) return
    const wv = Number(w)
    const hv = Number(h)
    const wChanged = facts.width_mode === 'px' && Number.isFinite(wv) && wv >= 1 && Math.round(bbox[2]) !== wv
    const hChanged = facts.height_mode === 'px' && Number.isFinite(hv) && hv >= 1 && Math.round(bbox[3]) !== hv
    // ONE session edit per commit: session editors read the armed document,
    // which only advances after a re-arm — two calls in one handler would
    // both edit the same base and the second would discard the first.
    if (wChanged && hChanged) edit((s) => s.setSize(selected, wv, hv))
    else if (wChanged) edit((s) => s.setDimension(selected, 'width', wv))
    else if (hChanged) edit((s) => s.setDimension(selected, 'height', hv))
  }

  const commitRadius = () => {
    const r = Number(radius)
    if (!Number.isFinite(r) || r < 0 || r === facts?.radius) return
    edit((s) => s.setRadius(selected, r))
  }

  const commitOpacity = () => {
    const o = Number(opacity)
    if (!Number.isFinite(o)) return
    const v = Math.max(0, Math.min(100, o)) / 100
    if (facts && Math.round(facts.opacity * 100) === Math.round(v * 100)) return
    edit((s) => s.setOpacity(selected, v))
  }

  const commitFill = (hexOverride?: string, alphaOverride?: string) => {
    if (!facts?.fill) return
    const rgb = hexToRgb(hexOverride ?? rgbToHex(facts.fill))
    const a = Number(alphaOverride ?? alpha)
    if (!rgb || !Number.isFinite(a)) return
    edit((s) => s.setFill(selected, rgb[0], rgb[1], rgb[2], Math.max(0, Math.min(100, a)) / 100))
  }

  const enterCommits = (commit: () => void) => (e: React.KeyboardEvent) => {
    if (e.key === 'Enter') commit()
  }

  const commitExpr = () => {
    if (!facts?.text || expr === facts.text.expr) return
    edit((s) => s.setTextExpr(selected, expr))
  }

  const commitFontSize = () => {
    const v = Number(fontSize)
    if (!Number.isFinite(v) || v < 1 || v === facts?.text?.size) return
    edit((s) => s.setFontSize(selected, v))
  }

  const commitFont = (familyOverride?: string, weightOverride?: number) => {
    const fam = (familyOverride ?? family).trim()
    const wt = weightOverride ?? Number(weight)
    if (!facts?.text || !fam || !Number.isFinite(wt)) return
    if (fam === facts.text.family && wt === facts.text.weight) return
    // The nearest face in the picked family carries the hash — that is the
    // file a node will stage for this declaration.
    const match = faces
      .filter((f) => f.family === fam)
      .toSorted((a, b) => Math.abs(a.weight - wt) - Math.abs(b.weight - wt))[0]
    edit((s) =>
      s.setFontFamily(selected, fam, Math.max(1, Math.min(1000, Math.round(wt))), match?.hash),
    )
  }

  return (
    <div className="design-console__selected">
      <GroupHeader label="Selected" trailing={selected} />
      {bbox && facts ? (
        <>
          <KeyValueRow label="Position" value={`${Math.round(bbox[0])}, ${Math.round(bbox[1])}`} tone="plain" />

          {facts.kind === 'mask' ? (
            // A mask draws nothing, so selecting one and finding no colours is
            // exactly right and needs saying once, here.
            <p className="design-console__dim">
              A mask reveals what falls inside its shape and hides the rest — everything
              beside it in its container. It has no colour of its own: size it, round it,
              and animate its position for a wipe.
            </p>
          ) : null}

          {/* The pin. Its own explicit control, because it changes what a
              drag DOES: pinned moves freely, in-flow moves shift neighbours. */}
          <FormGroup label="Placement">
            <select
              value={facts.absolute ? 'absolute' : 'relative'}
              aria-label="Placement"
              onChange={(e) => edit((s) => s.setPosition(selected, e.target.value === 'absolute'))}
            >
              <option value="relative">In the flow — neighbours move with it</option>
              <option value="absolute">Pinned — moves freely</option>
            </select>
          </FormGroup>

          {(() => {
            const pictureControls = facts.image ? (
              <div className="design-console__size">
                <FormGroup label="Picture">
                  <select
                    value={facts.image.hash ?? ''}
                    aria-label="Picture asset"
                    onChange={(e) => { if (e.target.value) edit((s) => s.setImage(selected, e.target.value)) }}
                  >
                    {facts.image.hash && !imageAssets.some((a) => a.hash === facts.image!.hash) ? (
                      <option value={facts.image.hash}>
                        {shortRev(facts.image.hash)} — not on this server
                      </option>
                    ) : null}
                    {imageAssets.map((a) => (
                      <option key={a.hash} value={a.hash}>
                        {shortRev(a.hash)} · {Math.max(1, Math.round(a.size / 1024))} KB
                      </option>
                    ))}
                  </select>
                </FormGroup>
                <FormGroup label="Fit">
                  <select
                    value={facts.image.fit}
                    aria-label="Picture fit"
                    onChange={(e) => edit((s) => s.setFit(selected, e.target.value))}
                  >
                    <option value="cover">Scale to fill — crops</option>
                    <option value="contain">Scale to fit — letterboxes</option>
                    <option value="stretch">Stretch — distorts</option>
                  </select>
                </FormGroup>
              </div>
            ) : null
            // An image NODE's picture is the point, not an option; only a
            // box's background image is a style it may or may not have.
            if (facts.kind !== 'box') return pictureControls
            if (!facts.image && imageAssets.length === 0) return null
            return (
              <OptionalSection
                label="Background image"
                on={!!facts.image}
                onEnable={() => edit((s) => s.setImage(selected, imageAssets[0]!.hash))}
                onDisable={() => edit((s) => s.clearImage(selected))}
              >
                {pictureControls}
              </OptionalSection>
            )
          })()}

          {facts.video ? (
            <>
              {/* A clip slot. The FIELD is the whole point: the template says
                  a video goes here, and every Item picks its own clip in Edit.
                  The canvas shows a dark plate — the browser cannot decode
                  broadcast codecs, and the motion is the node's job. */}
              <div className="design-console__size">
                <FormGroup label="Clip comes from field">
                  <input
                    value={facts.video.field ? '${' + facts.video.field + '}' : '— fixed asset —'}
                    readOnly
                    aria-label="Video field"
                  />
                </FormGroup>
                <FormGroup label="Fit">
                  <select
                    value={facts.video.fit}
                    aria-label="Video fit"
                    onChange={(e) => edit((s) => s.setFit(selected, e.target.value))}
                  >
                    <option value="cover">Scale to fill — crops</option>
                    <option value="contain">Scale to fit — letterboxes</option>
                    <option value="stretch">Stretch — distorts</option>
                  </select>
                </FormGroup>
              </div>
              <FormGroup label="At the end">
                <select
                  value={facts.video.looping ? 'loop' : 'hold'}
                  aria-label="Video end behaviour"
                  onChange={(e) => edit((s) => s.setLoop(selected, e.target.value === 'loop'))}
                >
                  <option value="hold">Hold the last frame</option>
                  <option value="loop">Loop</option>
                </select>
              </FormGroup>
            </>
          ) : null}

          {facts.text ? (
            <>
              {/* What it says. ${field} references bind Item data; typing one
                  declares the field, so publish never trips over bookkeeping
                  the designer was never shown. */}
              <FormGroup label="Text — ${field} binds Item data">
                <input
                  value={expr}
                  onChange={(e) => setExpr(e.target.value)}
                  onBlur={commitExpr}
                  onKeyDown={enterCommits(commitExpr)}
                  aria-label="Text content"
                />
              </FormGroup>
              <div className="design-console__size">
                <FormGroup label="Font family">
                  {/* A dropdown of what the server's font files call
                      THEMSELVES. Picking one carries the file's weight and
                      its hash into the declaration — the exact bytes a node
                      will stage. */}
                  <select
                    value={family}
                    disabled={!!facts.text?.token}
                    aria-label="Font family"
                    onChange={(e) => {
                      const fam = e.target.value
                      // Of the picked family's faces, keep the weight closest
                      // to what the node already has — switching family should
                      // not also silently switch Regular to Bold.
                      const wt = Number(weight) || 400
                      const face = faces
                        .filter((f) => f.family === fam)
                        .toSorted((a, b) => Math.abs(a.weight - wt) - Math.abs(b.weight - wt))[0]
                      setFamily(fam)
                      if (face) setWeight(String(face.weight))
                      commitFont(fam, face?.weight)
                    }}
                  >
                    {family && !faces.some((f) => f.family === family) ? (
                      <option value={family}>{family} — no file on this server</option>
                    ) : null}
                    {[...new Set(faces.map((f) => f.family))].map((f) => (
                      <option key={f} value={f}>{f}</option>
                    ))}
                  </select>
                </FormGroup>
                {theme && theme.fontTokens.length ? (
                  <TokenSelect
                    label="Font token"
                    tokens={theme.fontTokens}
                    value={facts.text?.token ?? null}
                    onPick={(t) => edit((s) => s.setFontToken(selected, t ?? '', theme.json))}
                  />
                ) : null}
                <FormGroup label="Weight">
                  <input
                    type="number"
                    min={1}
                    max={1000}
                    step={100}
                    value={weight}
                    disabled={!!facts.text?.token}
                    onChange={(e) => setWeight(e.target.value)}
                    onBlur={() => commitFont()}
                    onKeyDown={enterCommits(() => commitFont())}
                    aria-label="Font weight"
                  />
                </FormGroup>
              </div>
              <div className="design-console__size">
                <FormGroup label="Size">
                  <input
                    type="number"
                    min={1}
                    value={fontSize}
                    onChange={(e) => setFontSize(e.target.value)}
                    onBlur={commitFontSize}
                    onKeyDown={enterCommits(commitFontSize)}
                    aria-label="Font size"
                  />
                </FormGroup>
                <FormGroup label="Alignment">
                  <select
                    value={facts.text.align}
                    aria-label="Text alignment"
                    onChange={(e) => edit((s) => s.setTextAlign(selected, e.target.value))}
                  >
                    <option value="start">Left</option>
                    <option value="center">Centred</option>
                    <option value="end">Right</option>
                  </select>
                </FormGroup>
              </div>
            </>
          ) : null}

          {/* Per-axis sizing: a fixed number, or hugging what is inside —
              which is how a band grows with its name field while its height
              holds. The number shows the SOLVED size either way; it is only
              editable when the axis is fixed. */}
          <div className="design-console__size">
            <FormGroup label="Width">
              <div className="design-console__axis">
                <input
                  type="number"
                  min={1}
                  value={w}
                  disabled={facts.width_mode !== 'px'}
                  onChange={(e) => setW(e.target.value)}
                  onBlur={commitSize}
                  onKeyDown={enterCommits(commitSize)}
                  aria-label="Selected width"
                />
                <select
                  value={facts.width_mode}
                  aria-label="Width sizing"
                  onChange={(e) =>
                    edit((s) =>
                      s.setDimension(
                        selected,
                        'width',
                        e.target.value === 'px' ? Math.max(1, Math.round(bbox?.[2] ?? 1)) : undefined,
                      ),
                    )
                  }
                >
                  <option value="px">Fixed</option>
                  <option value="content">Fit contents</option>
                  {facts.width_mode === 'percent' ? (
                    // Shown only when the node IS a percent — a mask arrives
                    // at 100% of its parent. Choosing anything else leaves
                    // percent behind, which is why it is not offered outright.
                    <option value="percent">Percent of parent</option>
                  ) : null}
                </select>
              </div>
            </FormGroup>
            <FormGroup label="Height">
              <div className="design-console__axis">
                <input
                  type="number"
                  min={1}
                  value={h}
                  disabled={facts.height_mode !== 'px'}
                  onChange={(e) => setH(e.target.value)}
                  onBlur={commitSize}
                  onKeyDown={enterCommits(commitSize)}
                  aria-label="Selected height"
                />
                <select
                  value={facts.height_mode}
                  aria-label="Height sizing"
                  onChange={(e) =>
                    edit((s) =>
                      s.setDimension(
                        selected,
                        'height',
                        e.target.value === 'px' ? Math.max(1, Math.round(bbox?.[3] ?? 1)) : undefined,
                      ),
                    )
                  }
                >
                  <option value="px">Fixed</option>
                  <option value="content">Fit contents</option>
                  {facts.width_mode === 'percent' ? (
                    // Shown only when the node IS a percent — a mask arrives
                    // at 100% of its parent. Choosing anything else leaves
                    // percent behind, which is why it is not offered outright.
                    <option value="percent">Percent of parent</option>
                  ) : null}
                </select>
              </div>
            </FormGroup>
          </div>

          {/* Spacing, typed. Margin is the same fact a drag edits — where the
              node sits relative to its flow position; padding is a
              container's inner breathing room, which is how text stops
              touching the edge of its band. */}
          {(['margin', 'padding'] as const).map((kind) =>
            kind === 'padding' && facts.kind !== 'box' ? null : (
              <FormGroup key={kind} label={kind === 'margin' ? 'Margin — top right bottom left' : 'Padding — top right bottom left'}>
                <div className="design-console__edges">
                  {(['top', 'right', 'bottom', 'left'] as const).map((edge, i) => (
                    <input
                      key={`${kind}:${edge}:${facts[kind][i]}`}
                      type="number"
                      step={1}
                      defaultValue={facts[kind][i]}
                      aria-label={`${kind} ${edge}`}
                      onBlur={(e) => {
                        const v = Number(e.target.value)
                        if (!Number.isFinite(v) || v === facts[kind][i]) return
                        edit((s) => s.setSpacing(selected, kind, edge, v))
                      }}
                    />
                  ))}
                </div>
              </FormGroup>
            ),
          )}

          {facts.kind === 'box' ? (
            <FormGroup label="Overflow">
              <select
                value={facts.clip ? 'hide' : 'show'}
                aria-label="Overflow"
                onChange={(e) => edit((s) => s.setClip(selected, e.target.value === 'hide'))}
              >
                <option value="show">Show — contents may spill out</option>
                <option value="hide">Hide — mask contents to this box</option>
              </select>
            </FormGroup>
          ) : null}

          {facts.kind === 'mask' ? null : facts.text ? (
            // The fill IS the text colour — not an option, the ink itself.
            facts.fill ? (
              <>
                <div className="design-console__size">
                  <FormGroup label="Fill">
                    <input
                      type="color"
                      value={rgbToHex(facts.fill)}
                      disabled={!!facts.fill_token}
                      onChange={(e) => commitFill(e.target.value)}
                      aria-label="Fill colour"
                    />
                  </FormGroup>
                  <FormGroup label="Fill opacity %">
                    <input
                      type="number"
                      min={0}
                      max={100}
                      value={alpha}
                      onChange={(e) => setAlpha(e.target.value)}
                      onBlur={() => commitFill()}
                      onKeyDown={enterCommits(() => commitFill())}
                      aria-label="Fill opacity"
                    />
                  </FormGroup>
                </div>
                {theme ? (
                  <TokenSelect
                    label="Fill token"
                    tokens={theme.colorTokens}
                    value={facts.fill_token ?? null}
                    onPick={(t) => edit((s) => s.setColorToken(selected, 'fill', t ?? '', theme.json))}
                  />
                ) : null}
              </>
            ) : null
          ) : (
            <OptionalSection
              label="Fill"
              on={!!facts.fill || !!facts.gradient}
              onEnable={() => edit((s) => s.setFill(selected, 1, 1, 1, 1))}
              onDisable={() => edit((s) => s.clearFill(selected))}
              offNote="An unfilled box draws nothing and stops being clickable on the canvas — select it from the layer tree instead."
            >
              <FormGroup label="Type">
                <select
                  value={facts.gradient ? 'gradient' : 'solid'}
                  aria-label="Fill type"
                  onChange={(e) => {
                    if (e.target.value === 'gradient' && !facts.gradient) {
                      // Default: the current colour fading to its own shadow,
                      // top to bottom — a band look, not a rainbow.
                      const c = facts.fill ?? [1, 1, 1, 1]
                      const dark: [number, number, number, number] = [c[0] * 0.35, c[1] * 0.35, c[2] * 0.35, c[3]]
                      edit((s) => s.setGradient(selected, 180, JSON.stringify([[c, 0], [dark, 1]])))
                    } else if (e.target.value === 'solid' && facts.gradient) {
                      const first = facts.gradient.stops[0]![0]
                      edit((s) => s.setFill(selected, first[0], first[1], first[2], first[3]))
                    }
                  }}
                >
                  <option value="solid">Solid colour</option>
                  <option value="gradient">Gradient</option>
                </select>
              </FormGroup>
              {facts.gradient ? (
                <GradientControls
                  gradient={facts.gradient}
                  onCommit={(angle, stops) =>
                    edit((s) => s.setGradient(selected, angle, JSON.stringify(stops)))
                  }
                />
              ) : facts.fill ? (
                <>
                  <div className="design-console__size">
                    <FormGroup label="Colour">
                      <input
                        type="color"
                        value={rgbToHex(facts.fill)}
                        disabled={!!facts.fill_token}
                        onChange={(e) => commitFill(e.target.value)}
                        aria-label="Fill colour"
                      />
                    </FormGroup>
                    <FormGroup label="Opacity %">
                      <input
                        type="number"
                        min={0}
                        max={100}
                        value={alpha}
                        onChange={(e) => setAlpha(e.target.value)}
                        onBlur={() => commitFill()}
                        onKeyDown={enterCommits(() => commitFill())}
                        aria-label="Fill opacity"
                      />
                    </FormGroup>
                  </div>
                  {theme ? (
                    <TokenSelect
                      label="Fill token"
                      tokens={theme.colorTokens}
                      value={facts.fill_token ?? null}
                      onPick={(t) => edit((s) => s.setColorToken(selected, 'fill', t ?? '', theme.json))}
                    />
                  ) : null}
                </>
              ) : null}
            </OptionalSection>
          )}

          {/* Outline and shadow, on everything with a silhouette. Boxes and
              images decorate their rounded rect; text rings and shadows its
              glyphs — the outline drawn OUTWARD there, because an inside
              stroke eats thin letterforms. */}
          {facts.kind === 'box' || facts.kind === 'image' || facts.kind === 'text' ? (
            <>
              <OptionalSection
                label="Outline"
                on={!!facts.stroke}
                onEnable={() => edit((s) => s.setStroke(selected, 1, 1, 1, 1, 2))}
                onDisable={() => edit((s) => s.clearStroke(selected))}
              >
                {facts.stroke ? (
                  <div className="design-console__size">
                    <FormGroup label="Colour">
                      <input
                        type="color"
                        value={rgbToHex(facts.stroke.color)}
                        disabled={!!facts.stroke.token}
                        aria-label="Outline colour"
                        onChange={(e) => {
                          const rgb = hexToRgb(e.target.value)
                          if (rgb) edit((s) => s.setStroke(selected, rgb[0], rgb[1], rgb[2], facts.stroke!.color[3], facts.stroke!.width))
                        }}
                      />
                    </FormGroup>
                    <FormGroup label="Width">
                      <input
                        type="number"
                        min={0.5}
                        step={0.5}
                        key={`sw:${facts.stroke.width}`}
                        defaultValue={facts.stroke.width}
                        aria-label="Outline width"
                        onBlur={(e) => {
                          const v = Number(e.target.value)
                          if (Number.isFinite(v) && v > 0 && v !== facts.stroke!.width) {
                            const c = facts.stroke!.color
                            edit((s) => s.setStroke(selected, c[0], c[1], c[2], c[3], v))
                          }
                        }}
                      />
                    </FormGroup>
                  </div>
                ) : null}
                {facts.stroke && theme ? (
                  <TokenSelect
                    label="Outline token"
                    tokens={theme.colorTokens}
                    value={facts.stroke.token ?? null}
                    onPick={(t) => edit((s) => s.setColorToken(selected, 'stroke', t ?? '', theme.json))}
                  />
                ) : null}
              </OptionalSection>

              <OptionalSection
                label="Shadow"
                on={!!facts.shadow}
                onEnable={() => edit((s) => s.setShadow(selected, 0, 0, 0, 0.6, 6, 6, 12))}
                onDisable={() => edit((s) => s.clearShadow(selected))}
              >
                {facts.shadow ? (
                  <>
                    <div className="design-console__size">
                      <FormGroup label="Colour">
                        <input
                          type="color"
                          value={rgbToHex(facts.shadow.color)}
                          disabled={!!facts.shadow.token}
                          aria-label="Shadow colour"
                          onChange={(e) => {
                            const rgb = hexToRgb(e.target.value)
                            const sh = facts.shadow!
                            if (rgb) edit((s) => s.setShadow(selected, rgb[0], rgb[1], rgb[2], sh.color[3], sh.dx, sh.dy, sh.blur))
                          }}
                        />
                      </FormGroup>
                      <FormGroup label="Opacity %">
                        <input
                          type="number"
                          min={0}
                          max={100}
                          key={`sa:${facts.shadow.color[3]}`}
                          defaultValue={Math.round(facts.shadow.color[3] * 100)}
                          aria-label="Shadow opacity"
                          onBlur={(e) => {
                            const v = Number(e.target.value)
                            if (!Number.isFinite(v)) return
                            const sh = facts.shadow!
                            const c = sh.color
                            edit((s) => s.setShadow(selected, c[0], c[1], c[2], Math.max(0, Math.min(100, v)) / 100, sh.dx, sh.dy, sh.blur))
                          }}
                        />
                      </FormGroup>
                    </div>
                    <div className="design-console__size">
                      {(['dx', 'dy', 'blur'] as const).map((k) => (
                        <FormGroup key={k} label={k === 'dx' ? 'Offset X' : k === 'dy' ? 'Offset Y' : 'Softness'}>
                          <input
                            type="number"
                            step={1}
                            min={k === 'blur' ? 0 : undefined}
                            key={`${k}:${facts.shadow![k]}`}
                            defaultValue={facts.shadow![k]}
                            aria-label={`Shadow ${k}`}
                            onBlur={(e) => {
                              const v = Number(e.target.value)
                              if (!Number.isFinite(v) || v === facts.shadow![k]) return
                              const sh = { ...facts.shadow!, [k]: v }
                              const c = sh.color
                              edit((s) => s.setShadow(selected, c[0], c[1], c[2], c[3], sh.dx, sh.dy, sh.blur))
                            }}
                          />
                        </FormGroup>
                      ))}
                    </div>
                    {theme ? (
                      <TokenSelect
                        label="Shadow token"
                        tokens={theme.colorTokens}
                        value={facts.shadow.token ?? null}
                        onPick={(t) => edit((s) => s.setColorToken(selected, 'shadow', t ?? '', theme.json))}
                      />
                    ) : null}
                  </>
                ) : null}
              </OptionalSection>
            </>
          ) : null}

          <div className="design-console__size">
            <FormGroup label="Corner radius">
              <input
                type="number"
                min={0}
                value={radius}
                onChange={(e) => setRadius(e.target.value)}
                onBlur={commitRadius}
                onKeyDown={enterCommits(commitRadius)}
                aria-label="Corner radius"
              />
            </FormGroup>
            <FormGroup label="Opacity %">
              <input
                type="number"
                min={0}
                max={100}
                value={opacity}
                onChange={(e) => setOpacity(e.target.value)}
                onBlur={commitOpacity}
                onKeyDown={enterCommits(commitOpacity)}
                aria-label="Node opacity"
              />
            </FormGroup>
          </div>

          <p className="design-console__dim">
            Moving edits margins: a pinned node moves freely; one in the flow adjusts its
            place, so its neighbours move with it — that is the layout being responsive,
            not the editor being wrong.
          </p>

          {onDelete ? (
            <>
              <Button variant="secondary" block onClick={onDelete}>
                {facts.text
                  ? 'Delete this text'
                  : facts.kind === 'image'
                    ? 'Delete this image'
                    : facts.kind === 'mask'
                      ? 'Delete this mask — its container stops being masked'
                      : 'Delete this box — and everything inside it'}
              </Button>
              <p className="design-console__dim">Also on the keyboard: Delete, with the canvas focused.</p>
            </>
          ) : null}
        </>
      ) : null}
    </div>
  )
}

/**
 * The template's fields — the variables an Item author fills in Edit.
 *
 * The KEY is the contract: bindings say `${key}`, Items store values by it,
 * and it is deliberately not editable here — renaming a key would orphan both
 * sides silently, so a different key is a different field. Label, sample, and
 * required are the presentation half, and the sample is live: the canvas
 * renders with it, so editing it IS previewing it.
 */
function FieldsEditor({
  session,
  doc,
  onEdit,
}: {
  session: React.RefObject<DesignSession | null>
  doc: Record<string, unknown> | null
  onEdit: (doc: Record<string, unknown>) => void
}) {
  const fields =
    (doc?.fields as { key: string; label: string; sample?: string; required?: boolean }[] | undefined) ?? []
  const [newKey, setNewKey] = useState('')
  const [problem, setProblem] = useState<string | null>(null)

  const run = (fn: (s: DesignSession) => string) => {
    const s = session.current
    if (!s) return
    setProblem(null)
    try {
      onEdit(JSON.parse(fn(s)) as Record<string, unknown>)
    } catch (e) {
      setProblem(e instanceof Error ? e.message : String(e))
    }
  }

  const addField = () => {
    const key = newKey.trim()
    if (!key) return
    run((s) => s.addField(key))
    setNewKey('')
  }

  return (
    <div className="design-console__fieldsec">
      <GroupHeader label="Fields" trailing={fields.length ? `${fields.length}` : undefined} />
      {fields.length === 0 ? (
        <p className="design-console__dim">
          No fields yet. A field is what an Item fills in — add one here, or type{' '}
          {'${key}'} into a text node and it declares itself.
        </p>
      ) : null}
      {fields.map((f) => (
        <div key={f.key} className="design-console__fieldrow">
          <div className="design-console__fieldhead">
            <span className="design-console__fieldkey">{'${' + f.key + '}'}</span>
            <label className="design-console__fieldreq">
              <input
                type="checkbox"
                checked={!!f.required}
                aria-label={`Field ${f.key} required`}
                onChange={(e) =>
                  run((s) => s.updateField(f.key, f.label, f.sample ?? undefined, e.target.checked))
                }
              />
              Required
            </label>
            <button
              type="button"
              className="edit-console__iconbtn"
              aria-label={`Delete field ${f.key}`}
              onClick={() => run((s) => s.deleteField(f.key))}
            >
              <Trash2 size={13} aria-hidden />
            </button>
          </div>
          <div className="design-console__size">
            <FormGroup label="Label">
              <input
                key={`l:${f.key}:${f.label}`}
                defaultValue={f.label}
                aria-label={`Field ${f.key} label`}
                onBlur={(e) =>
                  run((s) => s.updateField(f.key, e.target.value, f.sample ?? undefined, !!f.required))
                }
              />
            </FormGroup>
            <FormGroup label="Sample — shown on the canvas">
              <input
                key={`s:${f.key}:${f.sample ?? ''}`}
                defaultValue={f.sample ?? ''}
                aria-label={`Field ${f.key} sample`}
                onBlur={(e) =>
                  run((s) => s.updateField(f.key, f.label, e.target.value || undefined, !!f.required))
                }
              />
            </FormGroup>
          </div>
        </div>
      ))}
      {problem ? <p className="tl__problem" role="alert">{problem}</p> : null}
      <div className="design-console__fieldadd">
        <input
          placeholder="key, e.g. speaker"
          aria-label="New field key"
          value={newKey}
          onChange={(e) => setNewKey(e.target.value)}
          onKeyDown={(e) => { if (e.key === 'Enter') addField() }}
        />
        <button type="button" className="edit-console__addbtn" onClick={addField}>
          <Plus size={12} aria-hidden /> Add a field
        </button>
      </div>
    </div>
  )
}

/**
 * A revision, shortened for the eye. The full value is an address whose
 * algorithm prefix is an internal fact (docs/UI_TEXT.md bans it on screen);
 * eight characters is plenty to tell two revisions apart in a list.
 */
const shortRev = (rev: string) => rev.replace(/^[a-z0-9]+:/, '').slice(0, 8)

// ---------------------------------------------------------------------------
// Revisions — who is pinned where, and moving them forward with a real diff.

type RevisionField = { key: string; label?: string; required?: boolean; type?: string }
type Revision = {
  template_rev: string
  name: string
  min_reader: number
  manifest: {
    name: string
    min_reader: number
    fields: RevisionField[]
    timelines: string[]
    assets?: string[]
  }
  published_at: number
  published_by: string | null
  items: { id: string; label: string }[]
}

/**
 * What moving an Item from one revision to another actually changes, as
 * sentences. Manifest-level on purpose: the manifest is the contract an Item
 * binds against — fields, cue timelines, media — and the server validated it
 * at publish. A pixel diff would need renders; this is the diff that predicts
 * whether the Item still WORKS.
 */
function manifestDiff(from: Revision['manifest'], to: Revision['manifest']): string[] {
  const lines: string[] = []
  const ff = new Map(from.fields.map((f) => [f.key, f]))
  const tf = new Map(to.fields.map((f) => [f.key, f]))
  for (const [k, f] of tf) {
    if (!ff.has(k)) lines.push(`Adds field “${f.label ?? k}”${f.required ? ' — required, so empty Items will be refused' : ''}`)
  }
  for (const [k, f] of ff) {
    if (!tf.has(k)) lines.push(`Removes field “${f.label ?? k}” — its data stays on the Item but no longer renders`)
  }
  for (const [k, f] of tf) {
    const o = ff.get(k)
    if (o && (o.required !== f.required || (o.label ?? k) !== (f.label ?? k) || o.type !== f.type)) {
      lines.push(`Changes field “${f.label ?? k}”`)
    }
  }
  const ot = new Set(from.timelines)
  const nt = new Set(to.timelines)
  for (const t of to.timelines) if (!ot.has(t)) lines.push(`Adds timeline “${t}” — a new cue button in Show`)
  for (const t of from.timelines) if (!nt.has(t)) lines.push(`Removes timeline “${t}” — its cue button goes away`)
  const oa = new Set(from.assets ?? [])
  const na = new Set(to.assets ?? [])
  const addedA = [...na].filter((h) => !oa.has(h)).length
  const droppedA = [...oa].filter((h) => !na.has(h)).length
  if (addedA || droppedA) {
    lines.push(`Changes media: ${addedA} added, ${droppedA} no longer used — nodes stage the difference`)
  }
  if (to.min_reader > from.min_reader) lines.push(`Needs newer nodes — reader ${to.min_reader} or later`)
  if (to.name !== from.name) lines.push(`Renames the template to “${to.name}”`)
  if (!lines.length) lines.push('Same fields, timelines, and media — only the look changed.')
  return lines
}

const revDate = (ms: number) =>
  new Date(ms).toLocaleString(undefined, { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' })

/**
 * The revision history for the open template, and the one action it exists
 * for: moving Items pinned to older revisions onto head — with the diff shown
 * BEFORE the button, and per-Item refusals shown after, by name.
 */
function Revisions({ docId, headRev }: { docId: string; headRev: string }) {
  const { data } = useQuery<{ revisions: Revision[] }>(keys.templateRevisions(docId))
  const [open, setOpen] = useState(false)
  const [busy, setBusy] = useState(false)
  const [result, setResult] = useState<{ updated: { label: string }[]; refused: { label: string; problem: string }[] } | null>(null)

  const revisions = data?.revisions ?? []
  const head = revisions.find((r) => r.template_rev === headRev)
  const stale = revisions.filter((r) => r.template_rev !== headRev && r.items.length > 0)
  const staleItems = new Set(stale.flatMap((r) => r.items.map((i) => i.id)))
  // An Item can appear under an old revision AND under head (one destination
  // moved, another not); it still needs the update.
  for (const i of head?.items ?? []) if (!stale.some((r) => r.items.some((x) => x.id === i.id))) staleItems.delete(i.id)

  const update = async () => {
    setBusy(true)
    setResult(null)
    try {
      const r = await post<{ updated: { id: string; label: string }[]; refused: { id: string; label: string; problem: string }[] }>(
        `/api/templates/${docId}/update-items`,
        {},
      )
      setResult(r)
      invalidate(keys.items())
      invalidate(keys.templateRevisions(docId))
    } catch (e) {
      setResult({ updated: [], refused: [{ label: 'Update', problem: e instanceof ApiError ? e.message : String(e) }] })
    } finally {
      setBusy(false)
    }
  }

  if (!revisions.length) return null
  return (
    <>
      <GroupHeader label="Revisions" trailing={`${revisions.length}`} />
      <div className="revs">
        {revisions.slice(0, 5).map((r) => (
          <div key={r.template_rev} className="revs__row">
            <span className="revs__rev">{shortRev(r.template_rev)}</span>
            <span className="revs__date">{revDate(r.published_at)}</span>
            {r.template_rev === headRev ? <Tag tone="raised" size={10}>newest</Tag> : null}
            <span className="revs__count">
              {r.items.length ? (r.items.length === 1 ? '1 Item' : `${r.items.length} Items`) : ''}
            </span>
          </div>
        ))}
        {revisions.length > 5 ? (
          <p className="design-console__dim">and {revisions.length - 5} older</p>
        ) : null}
        {stale.length && head ? (
          <>
            <p className="revs__stale">
              {staleItems.size === 1 ? '1 Item renders an older revision.' : `${staleItems.size} Items render older revisions.`}
            </p>
            <Button variant="secondary" block onClick={() => { setResult(null); setOpen(true) }}>
              Update Items to the newest revision…
            </Button>
          </>
        ) : null}
      </div>

      {open && head ? (
        <div className="mdb" role="dialog" aria-modal="true" aria-label="Update Items" onClick={() => setOpen(false)}>
          <div className="mdb__panel" onClick={(e) => e.stopPropagation()}>
            <header className="mdb__header">
              <span className="mdb__title">Update Items to {shortRev(headRev)}</span>
            </header>
            <div className="mdb__body">
              {stale.map((r) => (
                <div key={r.template_rev} className="revs__group">
                  <GroupHeader
                    label={`${shortRev(r.template_rev)} → ${shortRev(headRev)}`}
                    trailing={r.items.length === 1 ? '1 Item' : `${r.items.length} Items`}
                  />
                  <p className="revs__items">
                    {r.items.slice(0, 6).map((i) => i.label).join(' · ')}
                    {r.items.length > 6 ? ` · and ${r.items.length - 6} more` : ''}
                  </p>
                  <ul className="revs__diff">
                    {manifestDiff(r.manifest, head.manifest).map((line) => (
                      <li key={line}>{line}</li>
                    ))}
                  </ul>
                </div>
              ))}
              <p className="design-console__dim">
                Anything on air keeps playing the revision it was taken with; the update applies
                from the next take.
              </p>
              {result ? (
                <div className="revs__result" role="status">
                  {result.updated.length ? (
                    <p className="revs__ok">
                      {result.updated.length === 1
                        ? 'Moved 1 Item.'
                        : `Moved ${result.updated.length} Items.`}
                    </p>
                  ) : null}
                  {result.refused.map((f) => (
                    <p key={f.label} className="revs__refused">
                      {f.label} was not moved: {f.problem}
                    </p>
                  ))}
                  {!result.updated.length && !result.refused.length ? (
                    <p className="revs__ok">Everything is already on the newest revision.</p>
                  ) : null}
                </div>
              ) : null}
            </div>
            <footer className="revs__foot">
              <Button variant="secondary" disabled={busy} onClick={() => void update()}>
                {busy ? 'Updating…' : `Update ${staleItems.size === 1 ? '1 Item' : `${staleItems.size} Items`}`}
              </Button>
              <Button variant="quiet" onClick={() => setOpen(false)}>Close</Button>
            </footer>
          </div>
        </div>
      ) : null}
    </>
  )
}

/**
 * Start a template from nothing: a name and a canvas size, and the format
 * crate builds the document. The size presets are the outputs this system
 * actually drives — an LED wall is rarely 1080p, which is why this is not a
 * hidden default.
 */
const CANVAS_PRESETS: { label: string; w: number; h: number }[] = [
  { label: '1920 × 1080', w: 1920, h: 1080 },
  { label: '1280 × 720', w: 1280, h: 720 },
  { label: '3840 × 2160', w: 3840, h: 2160 },
]

function NewTemplate({ onCreate }: { onCreate: (name: string, w: number, h: number) => void }) {
  const [open, setOpen] = useState(false)
  const [name, setName] = useState('')
  const [preset, setPreset] = useState(0)
  const [custom, setCustom] = useState<{ w: number; h: number }>({ w: 1920, h: 1080 })
  const isCustom = preset === CANVAS_PRESETS.length

  const create = () => {
    const size = isCustom ? custom : (CANVAS_PRESETS[preset] ?? CANVAS_PRESETS[0]!)
    if (!(size.w >= 1 && size.h >= 1)) return
    onCreate(name.trim() || 'Untitled', Math.round(size.w), Math.round(size.h))
    setOpen(false)
    setName('')
  }

  if (!open) {
    return (
      <button type="button" className="design-console__file" onClick={() => setOpen(true)}>
        <Plus size={14} aria-hidden />
        <span>New template…</span>
      </button>
    )
  }
  return (
    <div className="design-console__new">
      <input
        autoFocus
        placeholder="Name"
        aria-label="New template name"
        value={name}
        onChange={(e) => setName(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === 'Enter') create()
          if (e.key === 'Escape') setOpen(false)
        }}
      />
      <select
        aria-label="Canvas size"
        value={preset}
        onChange={(e) => setPreset(Number(e.target.value))}
      >
        {CANVAS_PRESETS.map((p, i) => (
          <option key={p.label} value={i}>{p.label}</option>
        ))}
        <option value={CANVAS_PRESETS.length}>Custom…</option>
      </select>
      {isCustom ? (
        <div className="design-console__new-size">
          <input
            type="number"
            min={1}
            aria-label="Canvas width"
            value={custom.w}
            onChange={(e) => setCustom((c) => ({ ...c, w: Number(e.target.value) }))}
          />
          <span>×</span>
          <input
            type="number"
            min={1}
            aria-label="Canvas height"
            value={custom.h}
            onChange={(e) => setCustom((c) => ({ ...c, h: Number(e.target.value) }))}
          />
        </div>
      ) : null}
      <div className="design-console__new-actions">
        {/* Secondary, not primary: the console's red means ON AIR, and this
            makes a document, not a broadcast. */}
        <Button variant="secondary" onClick={create}>Create</Button>
        <Button variant="quiet" onClick={() => setOpen(false)}>Cancel</Button>
      </div>
    </div>
  )
}

export function Design() {
  const { data: templates } = useQuery<{ templates: TemplateSummary[] }>(keys.templates())
  const { data: assets } = useQuery<{ assets: Asset[] }>(keys.assets())
  const { data: items } = useQuery<{ items: Item[] }>(keys.items())
  const { data: themesData } = useQuery<{ themes: ThemeRow[] }>(keys.themes())
  const [themesOpen, setThemesOpen] = useState(false)

  const [selected, setSelected] = useState<string | null>(null)
  // The document, behind its undo history: `edit` for changes, `load` for
  // opening — a load starts a new timeline (ADR 0019: undo operates on the
  // SceneDoc, and lives where the document does).
  const { doc, load: loadDoc, edit: applyEdit, undo, redo, canUndo, canRedo } = useDocHistory()

  // Every edit marks the document dirty until a publish or a deliberate
  // discard — leaving with unpublished changes asks first (lib/dirty), here
  // and at the mode tabs. Undo does NOT clear it: an undone-then-abandoned
  // session still walked away from a document that differs from what was
  // opened, and guessing otherwise loses exactly the work the guard exists
  // to keep.
  const DIRTY = 'This template has unpublished changes. Leave and lose them?'
  const edit = useCallback(
    (d: Record<string, unknown>) => {
      applyEdit(d)
      setDirty(DIRTY)
    },
    [applyEdit],
  )
  const load = useCallback(
    (d: Record<string, unknown> | null) => {
      loadDoc(d)
      setDirty(null)
    },
    [loadDoc],
  )
  // Leaving the mode entirely unloads the document with it.
  useEffect(() => () => setDirty(null), [])
  const [data, setData] = useState<Record<string, string>>({})
  const [fonts, setFonts] = useState<Uint8Array[]>([])
  const [timeline, setTimeline] = useState<string | undefined>(undefined)
  const [tMs, setTMs] = useState(0)
  const [publishState, setPublishState] = useState<string | null>(null)
  const [mediaOpen, setMediaOpen] = useState(false)

  // Image bytes by hash, for the assets THIS document references. The cache
  // outlives the state so an undo that re-references a picture never
  // refetches it; the state only changes when the referenced set does, so an
  // unrelated edit never re-arms over identical bytes.
  const imageCache = useRef<Record<string, Uint8Array>>({})
  const imageKeys = useRef('')
  const [images, setImages] = useState<Record<string, Uint8Array>>({})
  useEffect(() => {
    const wanted = [
      ...new Set(
        ((doc?.assets as { hash: string; kind: string }[] | undefined) ?? [])
          .filter((a) => a.kind === 'image')
          .map((a) => a.hash),
      ),
    ].sort()
    let cancelled = false
    ;(async () => {
      await Promise.all(
        wanted
          .filter((h) => !imageCache.current[h])
          .map(async (h) => {
            const r = await fetch(`/cas/${h}`)
            if (r.ok) imageCache.current[h] = new Uint8Array(await r.arrayBuffer())
          }),
      )
      if (cancelled) return
      const key = wanted.filter((h) => imageCache.current[h]).join(',')
      if (key === imageKeys.current) return
      imageKeys.current = key
      setImages(Object.fromEntries(wanted.filter((h) => imageCache.current[h]).map((h) => [h, imageCache.current[h]!])))
    })().catch(() => {})
    return () => { cancelled = true }
  }, [doc])

  const canvasRef = useRef<HTMLCanvasElement | null>(null)
  const { status, problem, info, paint, session, generation } = useDesignCanvas(canvasRef, doc, data, fonts, images)
  // The selected NODE on the canvas — a different thing from the selected
  // template in the browser rail, which predates it.
  const [selNode, setSelNode] = useState<string | null>(null)

  const list = templates?.templates ?? []
  const fontAssets = (assets?.assets ?? []).filter((a) => FONT_KINDS.has(a.kind))
  const imageAssets = (assets?.assets ?? []).filter((a) => a.kind === 'image')

  // Load the fonts this server holds, and read each file's OWN name — the
  // family a picker offers must be what the file states, not a typed string.
  // One fetch per hash, and they are immutable, so this never needs
  // invalidating.
  const [faces, setFaces] = useState<FaceInfo[]>([])
  useEffect(() => {
    if (fontAssets.length === 0) return
    let cancelled = false
    Promise.all(
      fontAssets.map(async (a) => {
        const bytes = new Uint8Array(await (await fetch(`/cas/${a.hash}`)).arrayBuffer())
        const info = await fontFacts(bytes)
        return { bytes, face: info ? { hash: a.hash, ...info } : null }
      }),
    ).then((loaded) => {
      if (cancelled) return
      setFonts(loaded.map((l) => l.bytes))
      setFaces(loaded.map((l) => l.face).filter((f): f is FaceInfo => f !== null))
    }).catch(() => {})
    return () => { cancelled = true }
  }, [assets])

  // Load the selected template's published document, via its revision — which
  // IS its address in the CAS.
  const openTemplate = useCallback(async (id: string) => {
    // Opening another template replaces the document — same guard as leaving.
    if (!confirmLeave()) return
    setSelected(id)
    setSelNode(null)
    setPublishState(null)
    if (!id) { load(null); return }
    try {
      const t = await get<TemplateDetail>(keys.template(id))
      if (!t.head?.mirror_hash) {
        // The published revision is canonical CBOR, which this view cannot read
        // and deliberately never learns to — a JavaScript reader for the scene
        // format would be the second implementation of the thing that must
        // never disagree (ADR 0004). Without a mirror the file is the only way
        // back in, and saying so beats an empty canvas.
        load(null)
        setPublishState(
          'This template was published without its editable copy. Open the original file to edit it.',
        )
        return
      }
      const mirror = await (await fetch(`/cas/${t.head.mirror_hash}`)).json()
      // Older documents walk forward to the current format on open (§9.3);
      // publishing the migrated shape is still an explicit act.
      load(await migrateDoc(mirror as Record<string, unknown>))
      // Empty on purpose: arming falls back to each field's sample from the
      // DOCUMENT, so editing a sample in the fields editor shows on the canvas
      // immediately. A copy taken here would shadow those edits with the
      // values from load time.
      setData({})
    } catch (e) {
      setPublishState(e instanceof ApiError ? e.message : String(e))
    }
  }, [load])

  // After every re-arm, repaint the moment the designer is LOOKING AT — a key
  // edit while scrubbed to 300 ms must update that frame, not snap the canvas
  // back to rest. (The hook paints rest after arming; paint() queues the
  // latest request behind an in-flight one, so this wins.)
  useEffect(() => {
    if (status === 'ready' && (timeline !== undefined || tMs !== 0)) void paint(timeline, tMs)
    // Deliberately keyed on re-arms alone.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [generation])

  // Cmd/Ctrl+Z and Shift+Cmd/Ctrl+Z (plus Ctrl+Y), anywhere in Design except
  // inside a text control — a field keeps its own native undo, because taking
  // it over would make typing corrections rewind the document.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!(e.metaKey || e.ctrlKey)) return
      const key = e.key.toLowerCase()
      if (key !== 'z' && key !== 'y') return
      const t = e.target as HTMLElement | null
      if (t && ['INPUT', 'TEXTAREA', 'SELECT'].includes(t.tagName)) return
      e.preventDefault()
      if (key === 'y' || e.shiftKey) redo()
      else undo()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [undo, redo])

  // ---- Playback -------------------------------------------------------------
  // Real time from the current playhead to the end, painted by rAF. This is a
  // preview of the motion, not the node's clock — the node schedules frames
  // against its output; this schedules them against the browser, which is
  // exactly right for judging an ease and nothing more.
  const [playing, setPlaying] = useState(false)
  const tMsRef = useRef(0)
  tMsRef.current = tMs

  useEffect(() => {
    if (!playing) return
    const tl = ((doc?.timelines as DocTimeline[] | undefined) ?? []).find(
      (t) => t.name === timeline,
    )
    const duration = tl?.duration_ms ?? 0
    if (!timeline || duration <= 0 || status !== 'ready') {
      setPlaying(false)
      return
    }
    // Space at the end means "again", not a one-frame no-op.
    const from = tMsRef.current >= duration - 1 ? 0 : tMsRef.current
    const t0 = performance.now()
    let raf = 0
    const step = () => {
      const t = Math.min(duration, from + (performance.now() - t0))
      setTMs(t)
      // paint() keeps only the LATEST queued request, so rAF outpacing the
      // GPU drops intermediate frames rather than piling them up.
      void paint(timeline, t)
      if (t >= duration) {
        setPlaying(false)
        return
      }
      raf = requestAnimationFrame(step)
    }
    raf = requestAnimationFrame(step)
    return () => cancelAnimationFrame(raf)
    // tMs deliberately absent: the loop OWNS the playhead while it runs, and
    // keying on it would restart the clock every frame.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [playing, timeline, status, doc, paint])

  /** Space: play the selected timeline, or the first one when none is picked —
   *  the habit every editor teaches, honoured before it is unlearned. */
  const togglePlay = useCallback(() => {
    if (timeline) {
      setPlaying((p) => !p)
      return
    }
    const names = ((doc?.timelines as DocTimeline[] | undefined) ?? []).map((t) => t.name)
    if (names.length === 0) return
    const pick = names.includes('In') ? 'In' : names[0]!
    setTimeline(pick)
    setTMs(0)
    setPlaying(true)
  }, [timeline, doc])

  useEffect(() => {
    // Not while a dialog is up: space playing an animation BEHIND the Media
    // Browser is motion nobody asked for.
    if (mediaOpen) return
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== ' ' || e.metaKey || e.ctrlKey || e.altKey) return
      const t = e.target as HTMLElement | null
      // A focused button keeps its native space-click, and a text control
      // keeps its space. Everything else: space is transport, never scroll.
      if (
        t &&
        (['INPUT', 'TEXTAREA', 'SELECT', 'BUTTON'].includes(t.tagName) || t.isContentEditable)
      ) {
        return
      }
      e.preventDefault()
      togglePlay()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [togglePlay, mediaOpen])

  const onOpenFile = useCallback(async (file: File) => {
    if (!confirmLeave()) return
    setPublishState(null)
    try {
      const json = JSON.parse(await file.text()) as Record<string, unknown>
      load(await migrateDoc(json))
      setSelected(null)
      setSelNode(null)
      // Empty on purpose — see openTemplate: samples come from the document.
      setData({})
    } catch (e) {
      setPublishState(`That file is not a template: ${String(e)}`)
    }
  }, [load])

  // A blank document, built by the format crate through wasm — starting from
  // nothing must not require keeping a blank.json on disk somewhere. It exists
  // only in this session until the first publish, which is what creates it on
  // the server.
  const onNewTemplate = useCallback(async (name: string, width: number, height: number) => {
    if (!confirmLeave()) return
    setPublishState(null)
    try {
      load(await blankDoc(newId(), name, width, height))
      setSelected(null)
      setSelNode(null)
      setData({})
    } catch (e) {
      setPublishState(e instanceof Error ? e.message : String(e))
    }
  }, [load])

  const publish = useCallback(async () => {
    if (!doc) return
    setPublishState('Publishing…')
    try {
      // Compiled HERE, by the same Rust the CLI runs. The server stores the
      // bytes opaquely and checks only that their address matches the manifest.
      const compiled = await compileTemplate(doc)
      const id = String(doc.id ?? '')
      if (!list.some((t) => t.id === id)) {
        await post(keys.templates(), { id, name: String(doc.name ?? 'Untitled') })
      }
      const r = await post<{ template_rev: string; warnings: { code: string; detail: string }[]; missing_assets: string[] }>(
        `/api/templates/${id}/publish`,
        {
          scene: btoa(String.fromCharCode(...compiled.scene)),
          manifest: JSON.parse(compiled.manifest),
          // The editable copy travels with the publish, so this template can be
          // reopened on a machine that has never seen the original file.
          mirror: JSON.stringify(doc),
        },
      )
      invalidate(keys.templates())
      const bits = [`Published — revision ${shortRev(r.template_rev)}`]
      if (r.warnings.length) bits.push(`${r.warnings.length} warning(s)`)
      // Named here, not discovered by a node at staging time on a Saturday.
      if (r.missing_assets.length) bits.push(`${r.missing_assets.length} asset(s) this server does not have`)
      setPublishState(bits.join(' · '))
      // Published IS saved: the editable mirror travelled with it.
      setDirty(null)
    } catch (e) {
      setPublishState(e instanceof ApiError ? e.message : String(e))
    }
  }, [doc, list])

  const docId = doc ? String(doc.id ?? '') : null
  const usedBy = docId ? (items?.items ?? []).filter((i) => i.template_id === docId).length : 0
  const published = docId ? list.some((t) => t.id === docId && t.head_rev) : false
  const themes = themesData?.themes ?? []
  const docTheme = doc ? ((doc.theme as string | undefined) ?? null) : null
  // The theme the token pickers speak for: the one whose CURRENT head this
  // document was resolved under. A stale stamp offers a refresh instead.
  const activeTheme = useMemo(() => {
    const t = docTheme ? themes.find((x) => x.head_hash === docTheme) : undefined
    return t ? themeToActive(t) : null
  }, [docTheme, themes])

  // The right panel: template-level facts on one tab, the selected node on the
  // other. Selection drives the tab — clicking a node is asking about it, and
  // deselecting means the template is the subject again — but a manual tab
  // click sticks until the selection changes.
  const [panelTab, setPanelTab] = useState<'template' | 'inspector'>('template')
  useEffect(() => {
    setPanelTab(selNode ? 'inspector' : 'template')
  }, [selNode])
  const rootId = doc ? ((doc.root as { id?: string } | undefined)?.id ?? null) : null
  // The families this document declares, deduplicated — what the font picker
  // offers. Declaring a NEW one happens by typing it; the editor adds the
  // FontRef.
  /** Add a node — into the selection when there is one, else the canvas root —
   *  and select what was just made, so the next act is styling it. */
  const addNode = useCallback(
    (what: 'box' | 'text' | 'image' | 'video' | 'mask') => {
      const s = session.current
      if (!s) return
      try {
        const raw =
          what === 'box'
            ? s.addBox(selNode ?? undefined)
            : what === 'text'
              ? s.addText(selNode ?? undefined)
              : what === 'mask'
                ? s.addMask(selNode ?? undefined)
                : what === 'video'
                  ? s.addVideo(selNode ?? undefined)
                  : s.addImage(selNode ?? undefined, imageAssets[0]!.hash)
        const r = JSON.parse(raw) as { id: string; doc: Record<string, unknown> }
        edit(r.doc)
        setSelNode(r.id)
      } catch (e) {
        // A failed edit is the designer's to know about, in words.
        setPublishState(`Could not add a ${what}: ${e instanceof Error ? e.message : String(e)}`)
      }
    },
    [session, selNode, imageAssets],
  )

  /** Delete the selection and everything inside it. The root is the canvas
   *  and is never offered for deletion. */
  const deleteSelected = useCallback(() => {
    const s = session.current
    if (!s || !selNode || selNode === rootId) return
    try {
      edit(JSON.parse(s.deleteNode(selNode)) as Record<string, unknown>)
      setSelNode(null)
    } catch {
      // The node may already be gone; the selection effect clears it.
    }
  }, [session, selNode, rootId])

  // ---- The element clipboard and its menu ----------------------------------
  // In-app, not the system clipboard: the payload is document JSON and the
  // habit being served is cut/copy/paste WITHIN a template. Cut+paste keeps
  // ids (a move — timeline tracks follow); copy+paste re-ids the duplicate.
  const [clipboard, setClipboard] = useState<{ json: string; label: string } | null>(null)
  const [menu, setMenu] = useState<{ x: number; y: number; id: string | null } | null>(null)

  const copyEl = useCallback(
    (id: string) => {
      const s = session.current
      if (!s) return
      try {
        setClipboard({ json: s.copyNode(id), label: id })
      } catch {
        // Nothing to copy; the clipboard keeps what it had.
      }
    },
    [session],
  )

  const cutEl = useCallback(
    (id: string) => {
      const s = session.current
      if (!s || id === rootId) return
      try {
        const json = s.copyNode(id)
        edit(JSON.parse(s.deleteNode(id)) as Record<string, unknown>)
        setClipboard({ json, label: id })
        setSelNode((cur) => (cur === id ? null : cur))
      } catch {
        // Refused (the root); the sentence shows where delete's does.
      }
    },
    [session, rootId, edit],
  )

  const pasteEl = useCallback(
    (target: string | null, beside = false) => {
      const s = session.current
      if (!s || !clipboard) return
      try {
        const r = JSON.parse(s.pasteNode(target ?? '', clipboard.json, beside)) as {
          id: string
          doc: Record<string, unknown>
        }
        edit(r.doc)
        setSelNode(r.id)
      } catch (e) {
        setPublishState(e instanceof Error ? e.message : String(e))
      }
    },
    [session, clipboard, edit],
  )

  const duplicateEl = useCallback(
    (id: string) => {
      const s = session.current
      if (!s || id === rootId) return
      try {
        const r = JSON.parse(s.pasteNode(id, s.copyNode(id), true)) as {
          id: string
          doc: Record<string, unknown>
        }
        edit(r.doc)
        setSelNode(r.id)
      } catch (e) {
        setPublishState(e instanceof Error ? e.message : String(e))
      }
    },
    [session, rootId, edit],
  )

  // Cmd/Ctrl+C, X, V, D on elements — outside text controls, which keep their
  // native clipboard for words.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!(e.metaKey || e.ctrlKey) || e.shiftKey || e.altKey) return
      const k = e.key.toLowerCase()
      if (!['c', 'x', 'v', 'd'].includes(k)) return
      const t = e.target as HTMLElement | null
      if (t && (['INPUT', 'TEXTAREA', 'SELECT'].includes(t.tagName) || t.isContentEditable)) return
      if (k === 'v') {
        if (!clipboard) return
        e.preventDefault()
        pasteEl(selNode)
        return
      }
      if (!selNode) return
      e.preventDefault()
      if (k === 'c') copyEl(selNode)
      if (k === 'x') cutEl(selNode)
      if (k === 'd') duplicateEl(selNode)
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [selNode, clipboard, copyEl, cutEl, pasteEl, duplicateEl])

  const menuItems = useCallback(
    (id: string | null): MenuItem[] => {
      if (!id) {
        return [{ label: 'Paste', disabled: !clipboard, onPick: () => pasteEl(null) }]
      }
      const isRoot = id === rootId
      return [
        { label: 'Cut', disabled: isRoot, onPick: () => cutEl(id) },
        { label: 'Copy', onPick: () => copyEl(id) },
        { label: 'Paste', disabled: !clipboard, onPick: () => pasteEl(id) },
        { label: 'Duplicate', disabled: isRoot, onPick: () => duplicateEl(id) },
        'rule',
        {
          label: 'Delete',
          disabled: isRoot,
          danger: true,
          onPick: () => {
            const s = session.current
            if (!s) return
            try {
              edit(JSON.parse(s.deleteNode(id)) as Record<string, unknown>)
              setSelNode((cur) => (cur === id ? null : cur))
            } catch {
              // Refused; nothing changed.
            }
          },
        },
      ]
    },
    [clipboard, rootId, copyEl, cutEl, pasteEl, duplicateEl, session, edit],
  )

  return (
    <div className="design-console">
      <div className="console__toolbar">
        <div className="console__toolbar-group">
          <Button disabled={!doc} onClick={() => void publish()}>
            <Upload size={14} aria-hidden /> Publish
          </Button>
          <Button variant="secondary" disabled={status !== 'ready'} onClick={() => addNode('box')}>
            <Plus size={14} aria-hidden /> Add a box
          </Button>
          <Button variant="secondary" disabled={status !== 'ready'} onClick={() => addNode('text')}>
            <Type size={14} aria-hidden /> Add text
          </Button>
          <Button
            variant="secondary"
            disabled={status !== 'ready' || imageAssets.length === 0}
            title={imageAssets.length === 0 ? 'Add an image asset first — right rail' : undefined}
            onClick={() => addNode('image')}
          >
            <ImageIcon size={14} aria-hidden /> Add an image
          </Button>
          <Button variant="secondary" disabled={status !== 'ready'} onClick={() => addNode('video')}>
            <Clapperboard size={14} aria-hidden /> Add a video
          </Button>
          <Button
            variant="secondary"
            disabled={status !== 'ready'}
            title="A mask reveals what is inside its shape — animate it for a wipe"
            onClick={() => addNode('mask')}
          >
            <Scissors size={14} aria-hidden /> Add a mask
          </Button>
          <Button variant="secondary" onClick={() => setMediaOpen(true)}>
            <FolderOpen size={14} aria-hidden /> Media Browser
          </Button>
          <Button variant="secondary" disabled={!canUndo} onClick={undo} title="Undo (Cmd+Z)">
            <Undo2 size={14} aria-hidden /> Undo
          </Button>
          <Button variant="secondary" disabled={!canRedo} onClick={redo} title="Redo (Shift+Cmd+Z)">
            <Redo2 size={14} aria-hidden /> Redo
          </Button>
          <span className="console__toolbar-note">
            Publishing is what makes a Template usable by Items. Nodes stage it by its address.
          </span>
        </div>
        <Tag tone="raised" size={10}>{`${list.length} templates`}</Tag>
      </div>

      <div className="design-console__panels">
        <Panel width="var(--pm-rail-width-edit)" label="Template browser">
          <PanelHeader title="Template Browser" />
          <PanelBody>
            <NewTemplate onCreate={onNewTemplate} />
            <label className="design-console__file">
              <FileUp size={14} aria-hidden />
              <span>Open a template file…</span>
              <input
                type="file"
                accept=".json"
                onChange={(e) => { const f = e.target.files?.[0]; if (f) void onOpenFile(f) }}
              />
            </label>
            {list.length === 0 ? (
              <p className="design-console__dim">
                Nothing published yet. Open a template file and publish it.
              </p>
            ) : (
              <>
                <GroupHeader label="Published" trailing={`${list.length}`} />
                {list.map((t) => (
                  <NavItem
                    key={t.id}
                    title={t.name}
                    meta={t.head_rev ? `revision ${shortRev(t.head_rev)}` : 'no revision'}
                    active={selected === t.id}
                    onClick={() => void openTemplate(t.id)}
                  />
                ))}
              </>
            )}
          </PanelBody>
          {/* Structure below the browser, Figma-style: what is in THIS
              template, selectable and rearrangeable. Same selection as the
              canvas — one thing is selected, seen from two sides. */}
          <PanelHeader title="Layers" />
          <PanelBody>
            <LayerTree
              session={session}
              doc={doc}
              selected={selNode}
              onSelect={setSelNode}
              onEdit={edit}
              onMenu={(x, y, id) => setMenu({ x, y, id })}
            />
          </PanelBody>
        </Panel>

        <div className="console__center design-console__center">
          <div className="design-console__stage" aria-label="Canvas">
            {/* The checkerboard is CSS. The canvas itself is transparent, because
                the output is a key and the designer must see what will key.
                Geometry questions — what is under the pointer, where handles
                go, what a drag writes — are answered by Rust (ADR 0019); the
                stage draws the chrome at the coordinates it reports. */}
            <CanvasStage
              canvasRef={canvasRef}
              session={session}
              ready={status === 'ready'}
              generation={generation}
              selected={selNode}
              onSelect={setSelNode}
              onEdit={edit}
              onDelete={deleteSelected}
              onMenu={(x, y, id) => setMenu({ x, y, id })}
            />

            {status === 'idle' ? (
              <p className="design-console__dim">
                Open a template file, and add a font, to see it render.
              </p>
            ) : null}
            {status === 'loading' ? <p className="design-console__dim">Rendering…</p> : null}
            {problem ? (
              <p className="design-console__problem" role="alert">
                <AlertTriangle size={15} aria-hidden /> {problem}
              </p>
            ) : null}

            {info?.degraded ? (
              <p className="design-console__problem" role="status">
                <AlertTriangle size={15} aria-hidden />
                Part of this template can’t be drawn by this version. It shows as magenta.
              </p>
            ) : null}
            {info?.omitted.length ? (
              // This is *why* the band got shorter. Guessing is much worse than
              // being told.
              <p className="design-console__dim">Hidden by a rule: {info.omitted.join(', ')}</p>
            ) : null}
          </div>

          <div className="design-console__timeline">
            <TimelinePanel
              session={session}
              ready={status === 'ready'}
              doc={doc}
              selected={selNode}
              timeline={timeline}
              tMs={tMs}
              playing={playing}
              onTogglePlay={togglePlay}
              onPickTimeline={(v) => {
                setPlaying(false)
                setTimeline(v)
                void paint(v, tMs)
              }}
              onScrub={(t) => {
                // Scrubbing takes the transport back: predictable beats clever.
                setPlaying(false)
                setTMs(t)
                void paint(timeline, t)
              }}
              onEdit={edit}
            />
          </div>
        </div>

        <Panel width="var(--pm-inspector-width)" label="Template and publish">
          <div className="design-console__tabs" role="tablist" aria-label="Panel">
            {(['template', 'inspector'] as const).map((t) => (
              <button
                key={t}
                type="button"
                role="tab"
                aria-selected={panelTab === t}
                className={
                  panelTab === t
                    ? 'design-console__tab design-console__tab--on'
                    : 'design-console__tab'
                }
                onClick={() => setPanelTab(t)}
              >
                {t === 'template' ? 'Template' : 'Inspector'}
              </button>
            ))}
            <span className="design-console__tabs-spacer" />
            {doc ? (
              <StatusPill tone={published ? 'ready' : 'staged'} variant="subtle" size={10}>
                {published ? 'PUBLISHED' : 'NOT PUBLISHED'}
              </StatusPill>
            ) : null}
          </div>
          <PanelBody pad16>
            {doc ? (
              panelTab === 'template' ? (
                <>
                  <div className="design-console__name">
                    <span className="design-console__title">{String(doc.name ?? 'Untitled')}</span>
                    <span className="design-console__id">{docId}</span>
                  </div>

                  <KeyValueRow
                    label="Used by"
                    value={usedBy === 1 ? '1 Item' : `${usedBy} Items`}
                    tone={usedBy ? 'armed' : 'plain'}
                  />

                  {(() => {
                    const headRev = docId ? list.find((t) => t.id === docId)?.head_rev : null
                    return docId && headRev ? <Revisions docId={docId} headRev={headRev} /> : null
                  })()}

                  <GroupHeader label="Theme" />
                  <button
                    type="button"
                    className="design-console__file"
                    onClick={() => setThemesOpen(true)}
                  >
                    <FolderOpen size={14} aria-hidden />
                    <span>Themes — tokens, and applying them…</span>
                  </button>
                  {themes.length ? (
                    <>
                      <FormGroup label="Follows">
                        <select
                          value={activeTheme?.hash ?? docTheme ?? ''}
                          aria-label="Theme"
                          onChange={(e) => {
                            const s = session.current
                            if (!s) return
                            try {
                              if (!e.target.value) {
                                edit(JSON.parse(s.clearTheme()) as Record<string, unknown>)
                              } else {
                                const t = themes.find((x) => x.head_hash === e.target.value)
                                if (!t) return
                                const a = themeToActive(t)
                                edit(JSON.parse(s.applyTheme(a.json, a.hash)) as Record<string, unknown>)
                              }
                            } catch (err) {
                              setPublishState(err instanceof Error ? err.message : String(err))
                            }
                          }}
                        >
                          <option value="">No theme</option>
                          {docTheme && !activeTheme ? (
                            <option value={docTheme}>an older theme version</option>
                          ) : null}
                          {themes.map((t) => (
                            <option key={t.id} value={t.head_hash}>{t.name}</option>
                          ))}
                        </select>
                      </FormGroup>
                      {docTheme && !activeTheme ? (
                        <p className="design-console__dim">
                          This template was resolved under a theme version this server has moved
                          past — pick the theme again to update its values.
                        </p>
                      ) : null}
                    </>
                  ) : null}

                  <GroupHeader label="Sample data" />
                  {Object.keys(data).map((k) => (
                    <FormGroup key={k} label={k}>
                      <input
                        value={data[k] ?? ''}
                        onChange={(e) => setData((d) => ({ ...d, [k]: e.target.value }))}
                      />
                    </FormGroup>
                  ))}

                  <FieldsEditor session={session} doc={doc} onEdit={edit} />
                </>
              ) : selNode ? (
                <SelectedNode
                  session={session}
                  ready={status === 'ready'}
                  generation={generation}
                  selected={selNode}
                  faces={faces}
                  imageAssets={imageAssets}
                  theme={activeTheme}
                  onEdit={edit}
                  onDelete={selNode === rootId ? undefined : deleteSelected}
                />
              ) : (
                <p className="design-console__dim">
                  Nothing is selected. Click a box on the canvas or a layer in the tree —
                  drag to move, drag a handle to resize, arrows nudge, Alt ignores snapping.
                </p>
              )
            ) : (
              <p className="design-console__dim">
                Open a template from the browser or from a file. There is no canvas interaction
                yet — no selection, no moving a box, no property editing.
              </p>
            )}

            {publishState ? (
              <p className="design-console__status" role="status">
                <Upload size={14} aria-hidden /> {publishState}
              </p>
            ) : null}
          </PanelBody>
        </Panel>
      </div>

      {mediaOpen ? <MediaBrowser faces={faces} onClose={() => setMediaOpen(false)} /> : null}
      {themesOpen ? (
        <ThemeManager
          themes={themes}
          openDocId={docId}
          onClose={() => setThemesOpen(false)}
          onSwept={(ids) => {
            // The open template may have just been republished under the new
            // theme. Reload it so the editor shows what the server now holds —
            // unless there are unsaved edits, which the dirty guard protects.
            if (docId && ids.includes(docId)) void openTemplate(docId)
          }}
        />
      ) : null}
      {menu ? (
        <ContextMenu x={menu.x} y={menu.y} items={menuItems(menu.id)} onClose={() => setMenu(null)} />
      ) : null}
    </div>
  )
}
