/**
 * The rundown's state and actions, extracted from the panel so the console
 * layout can draw one rundown across three panels (rail, workspace, armed
 * inspector) without three copies of the rules.
 *
 * # Two different things, tracked separately, on purpose
 *
 * The **cursor** is where the operator is. What is **on air** is what the node
 * reports. They are allowed to disagree — someone takes a graphic from the
 * rack without touching the rundown, a GPI fires at the node, a cue is
 * rejected — and the console shows both marks rather than reconciling them.
 *
 * `docs/THEORY-OF-OPERATIONS.md`: *"If the server's rundown disagrees with what
 * the node reports, the UI shows the divergence and offers an explicit
 * reconcile — it never silently re-fires."*
 */

import { useCallback, useState } from 'react'
import { useQuery, invalidate } from '../../lib/useQuery'
import { useTopic } from '../../lib/stream'
import { keys, topics } from '../../lib/keys'
import { ApiError, newCueId, put, takeCue } from '../../lib/api'
import type { Item, LayersSnapshot } from './types'

export type Cursor = {
  rundown: string
  item: string | null
  index: number
  total: number
  label: string | null
  next: string | null
  previous: string | null
}

export type Playlist = { id: string; name: string; mode: string; items: SectionedItem[] }

/** An Item as it sits in a playlist, with the section that entry belongs to. */
export type SectionedItem = Item & { section?: string | null }

export type RundownDetail = {
  id: string
  name: string
  playlists: Playlist[]
  cursor: Cursor
}

export type RunningItem = SectionedItem & { playlist: string; mode: string }

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

export function useRundown({
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
  const running: RunningItem[] = (rundown?.playlists ?? []).flatMap((p) =>
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
  // the persistent indicator is the mitigation, not a decoration. It is never
  // a toggle on this screen: mode is a property of the content (ADR 0017).
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

  const moveTo = useCallback(
    async (itemId: string) => {
      await put(`${keys.rundown(rundownId)}/cursor`, { item: itemId })
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
  const jumpSection = useCallback(
    async (section: { name: string; item: string }) => {
      if (follow) fireAt(section.item)
      await put(`${keys.rundown(rundownId)}/cursor`, { section: section.name })
      invalidate(keys.rundown(rundownId))
    },
    [follow, fireAt, rundownId],
  )

  /**
   * Land on a specific entry. In Cue this arms — it moves the operator's place
   * and takes nothing. In Follow, navigating IS taking (ADR 0017), so landing
   * on a slide fires it.
   */
  const jumpItem = useCallback(
    (itemId: string) => {
      if (follow) fireAt(itemId)
      void moveTo(itemId)
    },
    [follow, fireAt, moveTo],
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
      const delta =
        e.key === 'ArrowDown' || e.key === 'ArrowRight' ? 1
        : e.key === 'ArrowUp' || e.key === 'ArrowLeft' ? -1
        : 0
      if (e.key === ' ' || e.key === 'Enter') {
        e.preventDefault()
        void takeNext()
        return
      }
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
    [known, reachable, follow, move, fireAt, cursor, takeNext],
  )

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
  const strangerLabels = strangers.map(
    (id) =>
      (layers?.layers ?? []).find((l) => l.occupant?.item_id === id)?.occupant?.item_label ?? id,
  )

  return {
    rundown,
    cursor,
    running,
    sections,
    at,
    mode,
    known,
    follow,
    onAir,
    layers,
    busy,
    problem,
    setProblem,
    strangerLabels,
    move,
    takeNext,
    fireAt,
    jumpSection,
    jumpItem,
    onKeyDown,
  }
}
