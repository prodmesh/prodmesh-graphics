/**
 * The keyframe timeline — chrome, not canvas (ADR 0019).
 *
 * A timeline is a list of tracks, each a list of keys with an easing: a list
 * widget, number inputs, and SVG. The EVALUATION is `pmg-anim`, reached
 * through the session — the "value here" this panel captures when adding a
 * key is the same number the renderer draws with, so the editor and the
 * pixels cannot disagree.
 *
 * # What it shows
 *
 * The selected node's six animatable properties, one row each, with keys as
 * diamonds on a shared ruler. Only the selected node: a full every-track view
 * is an overview problem for the layer-tree rung; while designing a move you
 * are animating THIS thing.
 *
 * # Every edit is a document edit
 *
 * Keys, durations, whole timelines — all go through the Rust editors and come
 * back as a new document, which means undo covers animation exactly as it
 * covers geometry, with no second history.
 */

import { useCallback, useEffect, useState } from 'react'
import { Pause, Play, Plus, Trash2 } from 'lucide-react'
import { Button, FormGroup, GroupHeader, Tag } from '../../components/console'
import type { DesignSession, DocTimeline, Ease } from './useDesign'

/** The properties a track may animate — the renderer's own list. */
const PROPS: { prop: string; label: string; fallback: number }[] = [
  { prop: 'transform.translate.x', label: 'move x', fallback: 0 },
  { prop: 'transform.translate.y', label: 'move y', fallback: 0 },
  { prop: 'transform.scale.x', label: 'scale x', fallback: 1 },
  { prop: 'transform.scale.y', label: 'scale y', fallback: 1 },
  { prop: 'transform.rotate', label: 'rotate', fallback: 0 },
  { prop: 'style.opacity', label: 'opacity', fallback: 1 },
]

/**
 * Named easings, the decelerating ease-out first after linear because it is
 * the broadcast signature: a band slides in fast and settles soft.
 */
const EASES: { name: string; ease: Ease }[] = [
  { name: 'Linear', ease: 'linear' },
  { name: 'Ease out', ease: { cubic: [0.16, 1, 0.3, 1] } },
  { name: 'Ease in', ease: { cubic: [0.5, 0, 0.75, 0] } },
  { name: 'Ease in-out', ease: { cubic: [0.65, 0, 0.35, 1] } },
]

const easeName = (e: Ease | undefined): string => {
  const enc = JSON.stringify(e ?? 'linear')
  return EASES.find((p) => JSON.stringify(p.ease) === enc)?.name ?? 'Custom'
}

/** SVG geometry: the ruler's drawable width in viewBox units per row. */
const W = 1000
const ROW_H = 18

export function TimelinePanel({
  session,
  ready,
  doc,
  selected,
  timeline,
  tMs,
  playing,
  onTogglePlay,
  onPickTimeline,
  onScrub,
  onEdit,
}: {
  session: React.RefObject<DesignSession | null>
  ready: boolean
  doc: Record<string, unknown> | null
  selected: string | null
  timeline: string | undefined
  tMs: number
  playing: boolean
  /** Space does the same thing, everywhere in Design outside a text control. */
  onTogglePlay: () => void
  onPickTimeline: (name: string | undefined) => void
  onScrub: (t: number) => void
  onEdit: (doc: Record<string, unknown>) => void
}) {
  const timelines = ((doc?.timelines as DocTimeline[] | undefined) ?? [])
  const active = timelines.find((t) => t.name === timeline)
  const duration = Math.max(1, active?.duration_ms ?? 1000)
  const [newName, setNewName] = useState('')
  const [keySel, setKeySel] = useState<{ prop: string; t: number } | null>(null)
  const [problem, setProblem] = useState<string | null>(null)

  // A different timeline or node is a different set of keys.
  useEffect(() => setKeySel(null), [timeline, selected])

  /** Run one session edit; failures are sentences, not silence. */
  const edit = useCallback(
    (fn: (s: DesignSession) => string) => {
      const s = session.current
      if (!s) return
      setProblem(null)
      try {
        onEdit(JSON.parse(fn(s)) as Record<string, unknown>)
      } catch (e) {
        setProblem(e instanceof Error ? e.message : String(e))
      }
    },
    [session, onEdit],
  )

  const addTimeline = () => {
    const name = newName.trim() || (timelines.some((t) => t.name === 'In') ? (timelines.some((t) => t.name === 'Out') ? `timeline-${timelines.length + 1}` : 'Out') : 'In')
    edit((s) => s.addTimeline(name, 700))
    setNewName('')
    onPickTimeline(name)
  }

  /**
   * A new key lands at the playhead carrying the value the renderer would
   * draw there — evaluated by pmg-anim through the session when the track
   * exists, the property's resting default when it does not. Continuity by
   * construction: adding a key never jolts the animation.
   */
  const addKey = (prop: string, fallback: number) => {
    const s = session.current
    if (!s || !selected || !timeline) return
    let v = fallback
    try {
      const values = JSON.parse(s.valuesAt(timeline, tMs)) as Record<string, number>
      v = values[`${selected}/${prop}`] ?? fallback
    } catch {
      // The fallback is the property's resting value; good enough to edit.
    }
    edit((s2) => s2.setKey(timeline, selected, prop, Math.round(tMs), v, JSON.stringify('linear')))
    setKeySel({ prop, t: Math.round(tMs) })
  }

  const selKey = (() => {
    if (!keySel || !active || !selected) return null
    const track = active.tracks.find((tr) => tr.target === `${selected}/${keySel.prop}`)
    const key = track?.keys.find((k) => Math.abs(k.t - keySel.t) <= 0.5)
    return key ? { ...keySel, key } : null
  })()

  const x = (t: number) => (t / duration) * W

  const scrubFrom = (e: React.PointerEvent<SVGSVGElement>) => {
    const r = e.currentTarget.getBoundingClientRect()
    const t = Math.max(0, Math.min(duration, ((e.clientX - r.left) / r.width) * duration))
    onScrub(Math.round(t))
  }

  if (!ready || !doc) return null

  return (
    <div className="tl">
      <div className="tl__bar">
        <GroupHeader label="Timeline" />
        <select
          value={timeline ?? ''}
          aria-label="Timeline"
          className="tl__pick"
          onChange={(e) => onPickTimeline(e.target.value || undefined)}
        >
          <option value="">At rest</option>
          {timelines.map((t) => (
            <option key={t.name} value={t.name}>{t.name}</option>
          ))}
        </select>

        {active ? (
          <>
            <button
              type="button"
              className="edit-console__iconbtn"
              aria-label={playing ? 'Pause' : 'Play'}
              title={playing ? 'Pause (Space)' : 'Play (Space)'}
              onClick={onTogglePlay}
            >
              {playing ? <Pause size={13} aria-hidden /> : <Play size={13} aria-hidden />}
            </button>
            <label className="tl__field">
              <span>Duration ms</span>
              <input
                type="number"
                min={1}
                key={`${active.name}:${active.duration_ms}`}
                defaultValue={Math.round(active.duration_ms)}
                aria-label="Timeline duration"
                onBlur={(e) => {
                  const ms = Number(e.target.value)
                  if (Number.isFinite(ms) && ms >= 1 && ms !== active.duration_ms) {
                    edit((s) => s.setTimelineDuration(active.name, ms))
                  }
                }}
              />
            </label>
            <span className="tl__readout">{Math.round(tMs)} ms</span>
            <button
              type="button"
              className="edit-console__iconbtn"
              aria-label={`Delete timeline ${active.name}`}
              onClick={() => {
                edit((s) => s.deleteTimeline(active.name))
                onPickTimeline(undefined)
              }}
            >
              <Trash2 size={13} aria-hidden />
            </button>
          </>
        ) : null}

        <span className="tl__spacer" />
        <input
          className="tl__name"
          placeholder="In"
          aria-label="New timeline name"
          value={newName}
          onChange={(e) => setNewName(e.target.value)}
          onKeyDown={(e) => { if (e.key === 'Enter') addTimeline() }}
        />
        <button type="button" className="edit-console__addbtn" onClick={addTimeline}>
          <Plus size={12} aria-hidden /> Add a timeline
        </button>
      </div>

      {problem ? <p className="tl__problem" role="alert">{problem}</p> : null}

      {active ? (
        selected ? (
          <div className="tl__tracks">
            <div className="tl__labels">
              {PROPS.map(({ prop, label, fallback }) => (
                <div key={prop} className="tl__label">
                  <span>{label}</span>
                  <button
                    type="button"
                    className="edit-console__iconbtn"
                    aria-label={`Add ${label} key at playhead`}
                    title="Add a key at the playhead"
                    onClick={() => addKey(prop, fallback)}
                  >
                    <Plus size={11} aria-hidden />
                  </button>
                </div>
              ))}
            </div>
            <svg
              className="tl__lanes"
              viewBox={`0 0 ${W} ${PROPS.length * ROW_H}`}
              preserveAspectRatio="none"
              onPointerDown={scrubFrom}
              onPointerMove={(e) => { if (e.buttons & 1) scrubFrom(e) }}
            >
              {PROPS.map((_, i) => (
                <line key={i} className="tl__lane" x1={0} y1={(i + 1) * ROW_H - 0.5} x2={W} y2={(i + 1) * ROW_H - 0.5} />
              ))}
              {/* 100 ms ticks, taller at 500. */}
              {Array.from({ length: Math.floor(duration / 100) + 1 }, (_, i) => (
                <line
                  key={i}
                  className={i % 5 === 0 ? 'tl__tick tl__tick--major' : 'tl__tick'}
                  x1={x(i * 100)} y1={0} x2={x(i * 100)} y2={PROPS.length * ROW_H}
                />
              ))}
              {PROPS.map(({ prop }, row) =>
                (active.tracks.find((tr) => tr.target === `${selected}/${prop}`)?.keys ?? []).map(
                  (k) => {
                    const isSel = selKey?.prop === prop && Math.abs(selKey.t - k.t) <= 0.5
                    const cx = x(k.t)
                    const cy = row * ROW_H + ROW_H / 2
                    return (
                      <path
                        key={`${prop}:${k.t}`}
                        className={isSel ? 'tl__key tl__key--selected' : 'tl__key'}
                        d={`M ${cx} ${cy - 5} L ${cx + 5} ${cy} L ${cx} ${cy + 5} L ${cx - 5} ${cy} Z`}
                        onPointerDown={(e) => {
                          e.stopPropagation()
                          setKeySel({ prop, t: k.t })
                          onScrub(Math.round(k.t))
                        }}
                      />
                    )
                  },
                ),
              )}
              <line className="tl__playhead" x1={x(tMs)} y1={0} x2={x(tMs)} y2={PROPS.length * ROW_H} />
            </svg>
          </div>
        ) : (
          <p className="tl__dim">Select a box on the canvas to see and set its keys.</p>
        )
      ) : timelines.length === 0 ? (
        <p className="tl__dim">
          No timelines yet. Add one — In and Out are the ones a take plays.
        </p>
      ) : null}

      {selKey && selected && timeline ? (
        <div className="tl__keybar">
          <Tag size={10} tone="armed">{PROPS.find((p) => p.prop === selKey.prop)?.label ?? selKey.prop}</Tag>
          <FormGroup label="Time ms">
            <input
              type="number"
              min={0}
              key={`t:${selKey.t}`}
              defaultValue={Math.round(selKey.t)}
              aria-label="Key time"
              onBlur={(e) => {
                const nt = Number(e.target.value)
                if (!Number.isFinite(nt) || nt < 0 || nt === selKey.t) return
                // ONE session operation. Composing delete+set here read the
                // same armed document twice and the set discarded the delete —
                // the key was duplicated, not moved.
                const { key, prop } = selKey
                edit((s) => s.retimeKey(timeline, selected, prop, key.t, nt))
                setKeySel({ prop, t: nt })
              }}
            />
          </FormGroup>
          <FormGroup label="Value">
            <input
              type="number"
              step="any"
              key={`v:${selKey.t}:${selKey.key.v}`}
              defaultValue={selKey.key.v}
              aria-label="Key value"
              onBlur={(e) => {
                const nv = Number(e.target.value)
                if (!Number.isFinite(nv) || nv === selKey.key.v) return
                edit((s) => s.setKey(timeline, selected, selKey.prop, selKey.t, nv, JSON.stringify(selKey.key.ease ?? 'linear')))
              }}
            />
          </FormGroup>
          <FormGroup label="Easing — from this key">
            <select
              value={easeName(selKey.key.ease)}
              aria-label="Key easing"
              onChange={(e) => {
                const preset = EASES.find((p) => p.name === e.target.value)
                if (!preset) return
                edit((s) => s.setKey(timeline, selected, selKey.prop, selKey.t, selKey.key.v, JSON.stringify(preset.ease)))
              }}
            >
              {EASES.map((p) => <option key={p.name} value={p.name}>{p.name}</option>)}
              {easeName(selKey.key.ease) === 'Custom' ? <option value="Custom">Custom</option> : null}
            </select>
          </FormGroup>
          <Button
            variant="secondary"
            onClick={() => {
              edit((s) => s.deleteKey(timeline, selected, selKey.prop, selKey.t))
              setKeySel(null)
            }}
          >
            Delete key
          </Button>
        </div>
      ) : null}
    </div>
  )
}
