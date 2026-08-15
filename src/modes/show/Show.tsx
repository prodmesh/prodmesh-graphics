/**
 * Show — for the person running it.
 *
 * This mode must be BORING. Large targets, no destructive action within reach
 * of a mis-tap, no route into Design, and state in the present tense —
 * "Lower Third · Pastor Dan", not "Item 4 active".
 *
 * # The take button must never lie
 *
 * `POST /api/cue` is synchronous and this view waits for it. There is
 * deliberately **no optimistic update**: a button that turns green before the
 * graphic exists is the precise failure the synchronous endpoint was built to
 * prevent. When the node does not confirm, the 503's `label` is rendered as a
 * sentence and stays on screen until the operator does something else.
 *
 * # Where the state on screen comes from
 *
 * Layer occupancy is read from `output:<id>:layers`, which the **node**
 * publishes. This view never infers what is on air from what it just asked for
 * — the node is the authority, and a browser that draws its own optimism is how
 * an operator ends up trusting a screen that disagrees with the programme feed.
 */

import { useCallback, useEffect, useState } from 'react'
import { AlertTriangle, Radio, Square, X } from 'lucide-react'
import { useTopic } from '../../lib/stream'
import { useQuery } from '../../lib/useQuery'
import { keys, topics } from '../../lib/keys'
import { ApiError, newCueId, takeCue } from '../../lib/api'
import type { Item, LayersSnapshot, NodeSummary, CueLogSnapshot } from './types'

const SHOW = 'default'

export function Show() {
  const { data: nodes } = useQuery<{ nodes: NodeSummary[] }>(keys.nodes())
  const { data: items } = useQuery<{ items: Item[] }>(keys.items())

  const node = nodes?.nodes?.[0]
  const output = node?.state?.outputs?.[0]

  return (
    <>
      <h1>Show</h1>
      {!node ? (
        <NoNodes />
      ) : (
        <div className="show">
          <OnAir nodeId={node.node_id} outputId={output?.id ?? 'out-0'} />
          <Rack
            nodeId={node.node_id}
            outputId={output?.id ?? 'out-0'}
            items={items?.items ?? []}
            reachable={node.reachable}
          />
          <TakeLog />
        </div>
      )}
    </>
  )
}

function NoNodes() {
  return (
    <div className="mode__stub">
      <p>No playout node has connected yet.</p>
      <p>
        Start one and point it at this server. Nothing here can go on air until a
        node is listening.
      </p>
    </div>
  )
}

// ---------------------------------------------------------------------------

function OnAir({ nodeId, outputId }: { nodeId: string; outputId: string }) {
  const layers = useTopic<LayersSnapshot>(topics.outputLayers(outputId))

  return (
    <section className="show__onair" aria-label="On air">
      <header className="show__onair-head">
        <Radio size={16} aria-hidden />
        <span>{nodeId}</span>
        <span className="show__dim">{outputId}</span>
        {layers?.fps ? <span className="show__dim">{layers.fps.toFixed(2)} fps</span> : null}
      </header>

      {!layers?.layers?.length ? (
        <p className="show__dim show__pad">Nothing on air.</p>
      ) : (
        <ul className="show__layers">
          {layers.layers.map((l) => (
            <li key={l.name} className={l.occupant ? 'show__layer show__layer--live' : 'show__layer'}>
              <span className="show__layer-name">{l.name}</span>
              {l.occupant ? (
                <>
                  {/* Present tense, and the graphic's own words — not an id. */}
                  <span className="show__layer-item">{l.occupant.item_label ?? l.occupant.item_id}</span>
                  <span className="show__dim">{l.occupant.phase}</span>
                  {l.occupant.degraded ? (
                    <span className="show__degraded" title="Part of this graphic could not be drawn">
                      <AlertTriangle size={13} aria-hidden /> degraded
                    </span>
                  ) : null}
                </>
              ) : (
                <span className="show__dim">clear</span>
              )}
            </li>
          ))}
        </ul>
      )}
    </section>
  )
}

// ---------------------------------------------------------------------------

function Rack({
  nodeId,
  outputId,
  items,
  reachable,
}: {
  nodeId: string
  outputId: string
  items: Item[]
  reachable: boolean
}) {
  const [busy, setBusy] = useState<string | null>(null)
  const [problem, setProblem] = useState<string | null>(null)

  const fire = useCallback(
    async (key: string, req: Parameters<typeof takeCue>[0]) => {
      setBusy(key)
      setProblem(null)
      try {
        await takeCue(req)
      } catch (e) {
        // The server's own sentence, verbatim. It already names the node and
        // says what is still on air; rewriting it here would lose that.
        setProblem(e instanceof ApiError ? e.message : String(e))
      } finally {
        setBusy(null)
      }
    },
    [],
  )

  return (
    <section className="show__rack" aria-label="Items">
      {problem ? (
        <p className="show__problem" role="alert">
          <AlertTriangle size={15} aria-hidden /> {problem}
        </p>
      ) : null}

      {!reachable ? (
        <p className="show__problem" role="status">
          {nodeId} is offline. Anything already on air is still on air.
        </p>
      ) : null}

      {items.length === 0 ? (
        <p className="show__dim show__pad">No items yet. Build one in Edit.</p>
      ) : (
        <ul className="show__items">
          {items.map((it) => (
            <ItemRow
              key={it.id}
              item={it}
              nodeId={nodeId}
              outputId={outputId}
              busy={busy}
              onFire={fire}
              disabled={!reachable}
            />
          ))}
        </ul>
      )}

      <div className="show__clear">
        <button
          type="button"
          className="btn btn--ghost"
          disabled={!reachable || busy !== null}
          onClick={() =>
            fire('clear-all', {
              cue_id: newCueId(),
              node: nodeId,
              output: outputId,
              op: 'clear_all',
              show: SHOW,
            })
          }
        >
          <X size={15} aria-hidden /> Clear everything
        </button>
      </div>
    </section>
  )
}

function ItemRow({
  item,
  nodeId,
  outputId,
  busy,
  onFire,
  disabled,
}: {
  item: Item
  nodeId: string
  outputId: string
  busy: string | null
  onFire: (key: string, req: Parameters<typeof takeCue>[0]) => void
  disabled: boolean
}) {
  const readiness = useTopic<{ ready: boolean; missing: string[] }>(topics.itemReadiness(item.id))
  const layer = item.destination?.layer ?? 'Lower Third'

  // Not "warns" — NOT ARMABLE. An item whose node has not finished staging its
  // assets cannot be taken, and the button says why rather than failing later
  // (ADR 0006).
  const staging = readiness ? !readiness.ready : false
  const blocked = disabled || staging

  return (
    <li className="show__item">
      <div className="show__item-id">
        <span className="show__item-label">{item.label}</span>
        <span className="show__dim">{layer}</span>
        {staging ? (
          <span className="show__staging">
            still copying{readiness?.missing?.length ? ` · ${readiness.missing.length} file(s)` : ''}
          </span>
        ) : null}
      </div>
      <div className="show__item-actions">
        <button
          type="button"
          className="btn btn--take"
          disabled={blocked || busy !== null}
          onClick={() =>
            // Minted HERE, at the press, and not regenerated on retry — that is
            // what makes a double-click one take rather than two.
            onFire(`take:${item.id}`, {
              cue_id: newCueId(),
              node: nodeId,
              output: outputId,
              layer,
              op: 'take',
              item: item.id,
              show: SHOW,
            })
          }
        >
          {busy === `take:${item.id}` ? 'Taking…' : 'Take'}
        </button>
        <button
          type="button"
          className="btn btn--ghost"
          disabled={disabled || busy !== null}
          onClick={() =>
            onFire(`out:${item.id}`, {
              cue_id: newCueId(),
              node: nodeId,
              output: outputId,
              layer,
              op: 'take_out',
              show: SHOW,
            })
          }
        >
          <Square size={13} aria-hidden /> Take out
        </button>
      </div>
    </li>
  )
}

// ---------------------------------------------------------------------------

/**
 * The take log.
 *
 * It arrives as the **snapshot of a log tail** — `{ head_seq, entries[] }` — not
 * as one event per frame, because the topic transport conflates and a
 * superseded event would be a lost one. Anything missed while the tab was
 * asleep is fetched over plain HTTP with `?since=` (ADR 0008).
 */
function TakeLog() {
  const tail = useTopic<CueLogSnapshot>(topics.cueLog(SHOW))
  const [gap, setGap] = useState<CueLogSnapshot['entries']>([])
  const [seen, setSeen] = useState(0)

  useEffect(() => {
    if (!tail) return
    const oldest = tail.entries[0]?.seq ?? tail.head_seq
    // A hole between what we last saw and the oldest entry in this frame means
    // conflation dropped frames. That is expected and fine — this is where the
    // pull half of the bridge earns its place.
    if (seen > 0 && oldest > seen + 1) {
      fetch(keys.cueLogSince(SHOW, seen))
        .then((r) => r.json())
        .then((g: CueLogSnapshot) => setGap(g.entries))
        .catch(() => {})
    }
    setSeen(tail.head_seq)
  }, [tail, seen])

  const entries = [...gap, ...(tail?.entries ?? [])]
    .filter((e, i, a) => a.findIndex((x) => x.seq === e.seq) === i)
    .toSorted((a, b) => b.seq - a.seq)
    .slice(0, 40)

  return (
    <section className="show__log" aria-label="Take log">
      <h2 className="show__log-head">Take log</h2>
      {entries.length === 0 ? (
        <p className="show__dim show__pad">Nothing has been taken yet.</p>
      ) : (
        <ol className="show__log-list">
          {entries.map((e) => (
            <li key={e.seq} className={e.result === 'applied' ? undefined : 'show__log-bad'}>
              <span className="show__log-time">
                {new Date(e.at_utc).toLocaleTimeString(undefined, { hour12: false })}
              </span>
              <span className="show__log-op">{e.op.replace('_', ' ')}</span>
              <span className="show__log-item">{e.item_label ?? e.layer ?? ''}</span>
              {/* Which surface fired it. Without this the log is baffling the
                  first time someone uses a GPI or Companion (ADR 0014). */}
              <span className="show__dim">{e.surface.replace('_', ' ')}</span>
            </li>
          ))}
        </ol>
      )}
    </section>
  )
}
