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

type Playlist = { id: string; name: string; mode: string; items: SectionedItem[] }

/** An Item as it sits in a playlist, with the section that entry belongs to. */
type SectionedItem = Item & { section?: string | null }

type RundownDetail = {
  id: string
  name: string
  playlists: Playlist[]
  cursor: Cursor
}

/**
 * The playout modes this build understands (ADR 0017).
 *
 * A mode it does not know is **refused loudly**, never treated as `cue`.
 * Guessing would mean an old browser silently giving an operator the arm step
 * in a playlist authored to fire on navigation — the keyboard doing something
 * other than what the content says it does, with nothing on screen to say so.
 * Same principle as a template whose reader version is too new.
 */
const MODES = ['cue', 'follow']

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
    p.items.map((it) => ({ ...it, playlist: p.name, mode: p.mode ?? 'cue' })),
  )

  // The song map. Sections in running order, de-duplicated, each remembering
  // the first entry it contains — because "go back to the chorus" means start
  // it again, not resume where it was left.
  const sections: { name: string; item: string }[] = []
  for (const it of running) {
    if (it.section && !sections.some((s) => s.name === it.section)) {
      sections.push({ name: it.section, item: it.id })
    }
  }

  // Mode belongs to the playlist the cursor is IN, so it changes as the
  // operator crosses a boundary mid-service. That is a real cognitive cost and
  // the indicator below is the mitigation, not a decoration.
  const at = running.find((i) => i.id === cursor?.item)
  const mode = at?.mode ?? 'cue'
  const known = MODES.includes(mode)
  const follow = known && mode === 'follow'

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

  /**
   * Fire a take without waiting for it (ADR 0017).
   *
   * Follow mode sends several cues a second. Awaiting each one would queue
   * blocking requests behind each other and grow latency the faster the
   * operator works — the opposite of what the mode is for. So this does not
   * await, and each ack resolves against the cue id it minted.
   *
   * **It sets nothing on air locally.** The on-air mark still comes from the
   * node, exactly as in Cue mode. Optimistic local state is the one thing this
   * ADR rejects outright: it is the take button lying, in the mode where the
   * operator is heads-down on the music and will never see the correction.
   *
   * A cue the node supersedes resolves 200 and is deliberately silent — that is
   * the mechanism working, and a message per skipped slide during a chorus is
   * noise an operator would learn to ignore.
   */
  const fireAt = useCallback(
    (itemId: string | null) => {
      const item = itemId ? running.find((i) => i.id === itemId) : null
      if (!item) return
      takeCue({
        cue_id: newCueId(),
        node: nodeId,
        output: outputId,
        layer: item.destination?.layer ?? 'Lower Third',
        op: 'take',
        item: item.id,
        show,
      }).catch((e) => setProblem(e instanceof ApiError ? e.message : String(e)))
    },
    [running, nodeId, outputId, show],
  )

  /**
   * Jump to a named section.
   *
   * The reference rundown calls this required: *"the worship leader suddenly
   * decides to sing the Chorus again. Operator clicks the first slide in the
   * Chorus to go back."* Under time pressure, with the band playing — which is
   * why it is a row of targets rather than a list to scroll.
   *
   * In Follow the jump TAKES, because in that mode navigating is taking and a
   * jump that only moved the cursor would leave the previous slide on air while
   * the operator believed they had moved.
   */
  const jump = useCallback(
    async (section: { name: string; item: string }) => {
      if (follow) fireAt(section.item)
      await put(`${keys.rundown(rundownId)}/cursor`, { section: section.name })
      invalidate(keys.rundown(rundownId))
    },
    [follow, fireAt, rundownId],
  )

  /**
   * The keyboard, which is a function of the focused playlist's mode.
   *
   * In `cue` the arrows move the cursor and taking is a separate act. In
   * `follow` they move AND take — an operator following the band has no spare
   * action, and a beat late in a chorus is the whole problem.
   */
  const onKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (!known || !reachable) return
      const delta = e.key === 'ArrowDown' ? 1 : e.key === 'ArrowUp' ? -1 : 0
      if (!delta) return
      e.preventDefault()
      if (!follow) {
        void move(delta)
        return
      }
      // Where the cursor is ABOUT to be. Read from the cursor we already have
      // rather than awaiting the move, because waiting for the server to tell
      // us where we went is the latency this mode cannot afford.
      const next = delta > 0 ? cursor?.next : cursor?.previous
      fireAt(next ?? null)
      void move(delta)
    },
    [known, reachable, follow, move, fireAt, cursor],
  )

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
    // Focusable so the arrow keys reach it, and the handler is a function of
    // the mode. A rundown mixes playlists of both kinds — that is the normal
    // Sunday — so what the keyboard does changes as the cursor crosses a
    // boundary, and the indicator below is how an operator knows without
    // looking for it.
    <section className="rundown" aria-label="Rundown" tabIndex={0} onKeyDown={onKeyDown}>
      <header className="rundown__head">
        <h2>{rundown.name}</h2>
        {known ? (
          <span className={follow ? 'rundown__mode rundown__mode--follow' : 'rundown__mode'}>
            {follow ? 'Follow — arrows take' : 'Cue — arrows move'}
          </span>
        ) : null}
        <span className="show__dim">
          {cursor && cursor.index >= 0 ? `${cursor.index + 1} of ${cursor.total}` : `${running.length} items`}
        </span>
      </header>

      {sections.length > 1 ? (
        <div className="rundown__sections" role="group" aria-label="Sections">
          {sections.map((sec) => (
            <button
              key={sec.name}
              type="button"
              className={
                at?.section === sec.name
                  ? 'rundown__section rundown__section--here'
                  : 'rundown__section'
              }
              disabled={!reachable}
              onClick={() => void jump(sec)}
            >
              {sec.name}
            </button>
          ))}
        </div>
      ) : null}

      {!known ? (
        // Refused, not guessed at. Falling back to `cue` would give the
        // operator an arm step in a playlist authored to fire on navigation,
        // with nothing on screen saying the keyboard is not doing what the
        // content asked for.
        <p className="show__problem" role="alert">
          <AlertTriangle size={15} aria-hidden /> This playlist uses a playout
          mode this version does not know (“{mode}”). Its keyboard shortcuts are
          off until you update.
        </p>
      ) : null}

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
