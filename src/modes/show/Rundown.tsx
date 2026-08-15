/**
 * The rundown panel: the running order, the operator's place in it, and what is
 * actually on air.
 *
 * # Two different things, drawn differently, on purpose
 *
 * The **cursor** is where the operator is. What is **on air** is what the node
 * reports. They are allowed to disagree — someone takes a graphic from the rack
 * without touching the rundown, a GPI fires at the node, a cue is rejected —
 * and this panel shows both marks rather than reconciling them.
 *
 * `docs/THEORY-OF-OPERATIONS.md`: *"If the server's rundown disagrees with what
 * the node reports, the UI shows the divergence and offers an explicit
 * reconcile — it never silently re-fires."* A panel that moved the cursor to
 * follow the output, or re-fired to follow the cursor, would be exactly that
 * silent re-fire.
 */

import { useCallback, useState } from 'react'
import { AlertTriangle, ChevronDown, ChevronUp, Play, Radio } from 'lucide-react'
import { useQuery, invalidate } from '../../lib/useQuery'
import { useTopic } from '../../lib/stream'
import { keys, topics } from '../../lib/keys'
import { ApiError, newCueId, put, takeCue } from '../../lib/api'
import type { Item, LayersSnapshot } from './types'

type Cursor = {
  rundown: string
  item: string | null
  index: number
  total: number
  label: string | null
  next: string | null
  previous: string | null
}

type RundownDetail = {
  id: string
  name: string
  playlists: { id: string; name: string; items: Item[] }[]
  cursor: Cursor
}

export function RundownPanel({
  rundownId,
  nodeId,
  outputId,
  reachable,
  show,
}: {
  rundownId: string
  nodeId: string
  outputId: string
  reachable: boolean
  show: string
}) {
  const { data: rundown } = useQuery<RundownDetail>(keys.rundown(rundownId))
  const live = useTopic<Cursor>(topics.rundownCursor(rundownId))
  const layers = useTopic<LayersSnapshot>(topics.outputLayers(outputId))

  const [busy, setBusy] = useState(false)
  const [problem, setProblem] = useState<string | null>(null)

  const cursor = live ?? rundown?.cursor
  const running = (rundown?.playlists ?? []).flatMap((p) =>
    p.items.map((it) => ({ ...it, playlist: p.name })),
  )

  // What the NODE says is showing, by item id. Not what we asked for.
  const onAir = new Set(
    (layers?.layers ?? []).map((l) => l.occupant?.item_id).filter(Boolean) as string[],
  )

  const move = useCallback(
    async (delta: number) => {
      await put(`${keys.rundown(rundownId)}/cursor`, { delta })
      invalidate(keys.rundown(rundownId))
    },
    [rundownId],
  )

  /**
   * Take what the cursor is on, then advance.
   *
   * Two separate operations, composed here because that is what an operator
   * wants from one button — and **the cursor only advances if the take
   * succeeded**. Advancing past a graphic that never went on air would leave
   * the operator's place and the output silently out of step.
   */
  const takeNext = useCallback(async () => {
    if (!cursor?.item) return
    const item = running.find((i) => i.id === cursor.item)
    if (!item) return
    setBusy(true)
    setProblem(null)
    try {
      await takeCue({
        cue_id: newCueId(),
        node: nodeId,
        output: outputId,
        layer: item.destination?.layer ?? 'Lower Third',
        op: 'take',
        item: item.id,
        show,
      })
      await move(1)
    } catch (e) {
      setProblem(e instanceof ApiError ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }, [cursor, running, nodeId, outputId, show, move])

  if (!rundown) return null

  // What counts as a divergence worth a sentence.
  //
  // NOT "the cursor isn't what's on air": Take next fires and then advances, so
  // the cursor is one ahead of the output almost all the time. A warning that
  // appears on every take is one an operator learns to ignore, which is worse
  // than no warning.
  //
  // The real signal is that the output is showing something **this rundown
  // does not contain** — taken from the rack, fired by a GPI, left over from
  // earlier. Where the operator is *within* the running order is already shown
  // by the two marks in the list, which needs no prose.
  const inRundown = new Set(running.map((i) => i.id))
  const strangers = [...onAir].filter((id) => !inRundown.has(id))
  const strangerLabels = strangers
    .map((id) => (layers?.layers ?? []).find((l) => l.occupant?.item_id === id)?.occupant?.item_label ?? id)

  return (
    <section className="rundown" aria-label="Rundown">
      <header className="rundown__head">
        <h2>{rundown.name}</h2>
        <span className="show__dim">
          {cursor && cursor.index >= 0 ? `${cursor.index + 1} of ${cursor.total}` : `${running.length} items`}
        </span>
      </header>

      {problem ? (
        <p className="show__problem" role="alert">
          <AlertTriangle size={15} aria-hidden /> {problem}
        </p>
      ) : null}

      <div className="rundown__transport">
        <button type="button" className="btn btn--ghost btn--small" onClick={() => void move(-1)}
                disabled={!cursor?.previous}>
          <ChevronUp size={14} aria-hidden /> Previous
        </button>
        <button type="button" className="btn btn--take" onClick={() => void takeNext()}
                disabled={!reachable || busy || !cursor?.item}>
          <Play size={14} aria-hidden /> {busy ? 'Taking…' : 'Take next'}
        </button>
        <button type="button" className="btn btn--ghost btn--small" onClick={() => void move(1)}
                disabled={!cursor?.next}>
          <ChevronDown size={14} aria-hidden /> Next
        </button>
      </div>

      {strangerLabels.length ? (
        <p className="rundown__diverged" role="status">
          On air from outside this rundown: {strangerLabels.join(', ')}.
        </p>
      ) : null}

      <ol className="rundown__list">
        {rundown.playlists.map((p) => (
          <li key={p.id} className="rundown__playlist">
            <h3>{p.name}</h3>
            <ol>
              {p.items.map((it) => {
                const atCursor = cursor?.item === it.id
                const showing = onAir.has(it.id)
                return (
                  <li
                    key={it.id}
                    className={[
                      'rundown__item',
                      atCursor ? 'rundown__item--cursor' : '',
                      showing ? 'rundown__item--live' : '',
                    ].filter(Boolean).join(' ')}
                  >
                    <button
                      type="button"
                      className="rundown__jump"
                      onClick={async () => {
                        // Jumping moves the operator's place. It does NOT take.
                        await put(`${keys.rundown(rundownId)}/cursor`, { item: it.id })
                        invalidate(keys.rundown(rundownId))
                      }}
                    >
                      {it.label}
                    </button>
                    {showing ? (
                      <span className="rundown__live" title="On air now">
                        <Radio size={12} aria-hidden /> on air
                      </span>
                    ) : null}
                  </li>
                )
              })}
            </ol>
          </li>
        ))}
      </ol>
    </section>
  )
}
