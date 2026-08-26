/**
 * Show — for the person running it.
 *
 * This mode must be BORING. Large targets, no destructive action within reach
 * of a mis-tap, no route into Design, and state in the present tense —
 * "Lower Third · Pastor Dan", not "Item 4 active".
 *
 * # The console layout (ADR 0020)
 *
 * Rundown rail on the left, the program well and layer strip in the centre,
 * the armed item and its commit on the right — so the operator reads what
 * they are about to send next to the button that sends it. The frame never
 * changes with the playlist; only the operator's controls do (ADR 0015).
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

import { useCallback, useEffect, useRef, useState } from 'react'
import { startH264Preview, previewSocketUrl, type PreviewStatus } from '../../lib/previewH264'
import { AlertTriangle, ListVideo, Target } from 'lucide-react'
import { useTopic } from '../../lib/stream'
import { useQuery } from '../../lib/useQuery'
import { keys, topics } from '../../lib/keys'
import { ApiError, newCueId, takeCue, type CueRequest } from '../../lib/api'
import {
  Button,
  GroupHeader,
  FormGroup,
  LayerCard,
  LogRow,
  Panel,
  PanelBody,
  PanelHeader,
  ProgramCanvas,
  RundownItem,
  SectionCard,
  SlideTile,
  StatusDot,
  StatusPill,
  Tag,
} from '../../components/console'
import { useRundown } from './useRundown'
import type {
  Item,
  LayersSnapshot,
  NodeSummary,
  CueLogSnapshot,
  Occupant,
  Readiness,
} from './types'

const SHOW = 'default'

/**
 * Which running order this station is on.
 *
 * Remembered per BROWSER, not on the server. A booth screen and a wall display
 * are different places and may honestly be on different shows; and inventing a
 * server-side "the live rundown" would be a piece of shared state nobody has
 * asked for, which the first two-service Sunday would then have to fight.
 */
const PICKED_RUNDOWN = 'pmg.rundown'

export function Show() {
  const { data: nodes } = useQuery<{ nodes: NodeSummary[] }>(keys.nodes())
  const { data: items } = useQuery<{ items: Item[] }>(keys.items())
  const { data: rundowns } = useQuery<{ rundowns: { id: string; name: string }[] }>(keys.rundowns())

  const [picked, setPicked] = useState<string | null>(() => {
    try {
      return window.localStorage.getItem(PICKED_RUNDOWN)
    } catch {
      return null
    }
  })

  const node = nodes?.nodes?.[0]
  const output = node?.state?.outputs?.[0]
  const all = rundowns?.rundowns ?? []
  // A remembered rundown that has since been deleted falls back rather than
  // leaving the console blank on the morning it is needed.
  const rundown = all.find((r) => r.id === picked) ?? all[0]

  const pick = (id: string) => {
    setPicked(id)
    try {
      window.localStorage.setItem(PICKED_RUNDOWN, id)
    } catch {
      /* a browser with storage off simply picks again next time */
    }
  }

  if (!node) return <NoNodes />

  return rundown ? (
    <RundownConsole
      key={rundown.id}
      rundownId={rundown.id}
      rundowns={all}
      onPick={pick}
      node={node}
      outputId={output?.id ?? 'out-0'}
      items={items?.items ?? []}
    />
  ) : (
    <RackConsole node={node} outputId={output?.id ?? 'out-0'} items={items?.items ?? []} />
  )
}

function NoNodes() {
  return (
    <div className="mode-scroll">
      <div className="mode__stub">
        <p>No playout node has connected yet.</p>
        <p>
          Start one and point it at this server. Nothing here can go on air until a node is
          listening.
        </p>
      </div>
    </div>
  )
}

// ---------------------------------------------------------------------------
// The rundown console: rail · program · armed. The normal Sunday.

function RundownConsole({
  rundownId,
  rundowns,
  onPick,
  node,
  outputId,
  items,
}: {
  rundownId: string
  rundowns: { id: string; name: string }[]
  onPick: (id: string) => void
  node: NodeSummary
  outputId: string
  items: Item[]
}) {
  const r = useRundown({
    rundownId,
    nodeId: node.node_id,
    outputId,
    reachable: node.reachable,
    show: SHOW,
  })

  const armed = r.at ? (items.find((i) => i.id === r.at!.id) ?? r.at) : null
  // By ENTRY, not by item: a chorus that comes round three times is in the
  // running order three times, and matching on the item finds the first one
  // (ADR 0026).
  const cursorPlaylist = r.rundown?.playlists.find((p) =>
    p.items.some((i) => i.entry_id === r.cursor?.entry),
  )

  if (!r.rundown) return null

  // A running position per entry, so the rail reads like a cue sheet.
  let position = 0
  const numbered = new Map<string, string>()
  for (const it of r.running) numbered.set(it.entry_id, String(++position).padStart(2, '0'))

  return (
    <div className="show-console">
      <div className="console__toolbar">
        <div className="console__toolbar-group">
          {/* One rundown is a tag; several is a choice. A booth that cannot
              reach next Sunday's running order has to be edited to get at it,
              which is the wrong mode for an operator. */}
          {rundowns.length > 1 ? (
            <select
              className="show-console__pick"
              value={rundownId}
              aria-label="Rundown"
              onChange={(e) => onPick(e.target.value)}
            >
              {rundowns.map((x) => <option key={x.id} value={x.id}>{x.name}</option>)}
            </select>
          ) : (
            <Tag pill tone="raised">{r.rundown.name}</Tag>
          )}
          {r.known ? (
            <Tag tone={r.follow ? 'armed' : 'quiet'} size={10}>
              {r.follow ? 'FOLLOW · TAKE ON NAV' : 'CUE · ARM ONLY'}
            </Tag>
          ) : null}
          <span className="console__toolbar-note">
            {r.follow
              ? 'Navigation takes items immediately.'
              : 'Arrows arm items. Press Take to go on air.'}
          </span>
        </div>
        <ClearAll nodeId={node.node_id} outputId={outputId} disabled={!node.reachable} onProblem={r.setProblem} />
      </div>

      <div className="show-console__panels">
        {/* The transport keys live on the WINDOW (useRundown), so they work
            regardless of what holds focus — an operator who clicked the layer
            rack still takes on space. What the keyboard does is a function of
            the mode; a rundown mixes playlists of both kinds — that is the
            normal Sunday — and the toolbar indicator is how an operator knows
            without looking for it. */}
        <Panel width="var(--pm-rail-width)">
          <section
            aria-label="Rundown"
            tabIndex={0}
            className="show-console__rail"
          >
            <PanelHeader
              title="Live Rundown"
              icon={<ListVideo size={16} aria-hidden color="var(--pm-white)" />}
              trailing={<Tag size={10}>{`${r.running.length} items`}</Tag>}
            />
            <PanelBody>
              {!r.known ? (
                // Refused, not guessed at. Falling back to `cue` would give the
                // operator an arm step in a playlist authored to fire on
                // navigation, with nothing on screen saying the keyboard is not
                // doing what the content asked for.
                <p className="show-console__problem" role="alert">
                  <AlertTriangle size={15} aria-hidden /> This playlist uses a playout mode this
                  version does not know (“{r.mode}”). Its keyboard shortcuts are off until you
                  update.
                </p>
              ) : null}

              {r.rundown.playlists.map((p) => (
                <div key={p.id} className="show-console__group">
                  <GroupHeader
                    label={p.name}
                    trailing={`${p.items.length} items · ${p.mode ?? 'cue'}`}
                  />
                  {p.items.map((it) => {
                    const atCursor = r.cursor?.entry === it.entry_id
                    const showing = r.onAir.has(it.id)
                    return (
                      <RundownItem
                        key={it.entry_id}
                        id={numbered.get(it.entry_id)}
                        title={it.label}
                        subtitle={it.section ?? undefined}
                        state={showing ? 'air' : atCursor ? 'armed' : 'idle'}
                        pill={
                          showing
                            ? { label: 'ON AIR', tone: 'air' }
                            : atCursor
                              ? { label: r.follow ? 'NEXT' : 'ARMED', tone: 'armed' }
                              : undefined
                        }
                        onClick={() => r.jumpEntry(it.entry_id)}
                      />
                    )
                  })}
                </div>
              ))}

              {r.strangerLabels.length ? (
                <p className="show-console__diverged" role="status">
                  On air from outside this rundown: {r.strangerLabels.join(', ')}.
                </p>
              ) : null}
            </PanelBody>
          </section>
        </Panel>

        <div className="console__center show-console__center">
          <div className="show-console__well">
            <ProgramWell node={node} outputId={outputId} layers={r.layers} />
          </div>
          <LayerStrip
            layers={r.layers}
            nodeId={node.node_id}
            outputId={outputId}
            reachable={node.reachable}
            onProblem={r.setProblem}
          />
          {cursorPlaylist && cursorPlaylist.items.length > 1 ? (
            <div className="show-console__grid-wrap">
              {r.sections.length > 1 ? (
                <div className="show-console__sections" role="group" aria-label="Sections">
                  {r.sections.map((sec) => (
                    <SectionCard
                      key={sec.name}
                      label={sec.name}
                      state={r.at?.section === sec.name ? 'HERE' : 'JUMP'}
                      active={r.at?.section === sec.name}
                      disabled={!node.reachable}
                      onClick={() => void r.jumpSection(sec)}
                    />
                  ))}
                </div>
              ) : null}
              <div className="show-console__grid">
                {cursorPlaylist.items.map((it, n) => {
                  const cursorIdx = cursorPlaylist.items.findIndex(
                    (x) => x.entry_id === r.cursor?.entry,
                  )
                  const state = r.onAir.has(it.id)
                    ? 'air'
                    : r.cursor?.entry === it.entry_id
                      ? 'armed'
                      : cursorIdx >= 0 && n < cursorIdx
                        ? 'past'
                        : 'upcoming'
                  return (
                    <SlideTile
                      key={it.entry_id}
                      num={String(n + 1).padStart(2, '0')}
                      line1={it.label}
                      line2={it.section ?? undefined}
                      state={state}
                      pill={
                        state === 'air'
                          ? { label: 'ON AIR', tone: 'air' }
                          : state === 'armed'
                            ? { label: r.follow ? 'NEXT' : 'ARMED', tone: 'armed' }
                            : undefined
                      }
                      disabled={!node.reachable}
                      onClick={() => r.jumpEntry(it.entry_id)}
                    />
                  )
                })}
              </div>
            </div>
          ) : null}
        </div>

        <Panel width="var(--pm-inspector-width)" label="Armed">
          <PanelHeader
            title="Armed"
            icon={<Target size={16} aria-hidden color="var(--pm-state-armed)" />}
            trailing={
              r.busy ? (
                <StatusPill tone="staged" variant="subtle" size={10}>WAITING</StatusPill>
              ) : armed ? (
                <StatusPill tone="armed" variant="subtle" size={10}>READY</StatusPill>
              ) : undefined
            }
          />
          <PanelBody pad16>
            {armed ? (
              <ArmedDetail
                item={armed}
                reachable={node.reachable}
                busy={r.busy}
                problem={r.problem}
                takeLabel={r.busy ? 'Taking…' : 'Take next'}
                takeNote={
                  r.follow
                    ? 'Arrow keys take. [Space] takes and advances.'
                    : 'Use Arrow Keys [↑/↓] to arm, [Space] to commit.'
                }
                onTake={() => void r.takeNext()}
                onAlso={(req) => takeCue(req).catch((e) => r.setProblem(e instanceof ApiError ? e.message : String(e)))}
                onTakeOut={(req) => takeCue(req).catch((e) => r.setProblem(e instanceof ApiError ? e.message : String(e)))}
                nodeId={node.node_id}
                outputId={outputId}
              />
            ) : (
              <p className="show-console__dim">Nothing is armed. Click a row or use the arrows.</p>
            )}
            <TakeLog />
          </PanelBody>
        </Panel>
      </div>
    </div>
  )
}

// ---------------------------------------------------------------------------
// The rack console: no rundown yet, so the rail lists the items themselves.

function RackConsole({
  node,
  outputId,
  items,
}: {
  node: NodeSummary
  outputId: string
  items: Item[]
}) {
  const layersSnap = useTopic<LayersSnapshot>(topics.outputLayers(outputId))
  const [selected, setSelected] = useState<string | null>(null)
  const [busy, setBusy] = useState<string | null>(null)
  const [problem, setProblem] = useState<string | null>(null)

  const armed = items.find((i) => i.id === selected) ?? items[0] ?? null
  const onAir = new Set(
    (layersSnap?.layers ?? []).map((l) => l.occupant?.item_id).filter(Boolean) as string[],
  )

  const fire = useCallback(async (key: string, req: CueRequest) => {
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
  }, [])

  const layers = [...new Set(items.map((it) => it.destination?.layer ?? 'Lower Third'))]

  return (
    <div className="show-console">
      <div className="console__toolbar">
        <div className="console__toolbar-group">
          <Tag pill tone="raised">Items</Tag>
          <span className="console__toolbar-note">
            No rundown yet. Build playlists and a rundown in Edit; until then every item is one
            take away.
          </span>
        </div>
        <ClearAll nodeId={node.node_id} outputId={outputId} disabled={!node.reachable} onProblem={setProblem} />
      </div>

      <div className="show-console__panels">
        <Panel width="var(--pm-rail-width)" label="Items">
          <PanelHeader
            title="Items"
            icon={<ListVideo size={16} aria-hidden color="var(--pm-white)" />}
            trailing={<Tag size={10}>{`${items.length} items`}</Tag>}
          />
          <PanelBody>
            {!node.reachable ? (
              <p className="show-console__problem" role="status">
                {node.name} is offline. Anything already on air is still on air.
              </p>
            ) : null}
            {items.length === 0 ? (
              <p className="show-console__dim">No items yet. Build one in Edit.</p>
            ) : (
              layers.map((layer) => (
                <div key={layer} className="show-console__group">
                  <GroupHeader label={layer} />
                  {items
                    .filter((it) => (it.destination?.layer ?? 'Lower Third') === layer)
                    .map((it) => (
                      <RackRow
                        key={it.id}
                        item={it}
                        armed={armed?.id === it.id}
                        showing={onAir.has(it.id)}
                        onClick={() => setSelected(it.id)}
                      />
                    ))}
                </div>
              ))
            )}
          </PanelBody>
        </Panel>

        <div className="console__center show-console__center">
          <div className="show-console__well">
            <ProgramWell node={node} outputId={outputId} layers={layersSnap} />
          </div>
          <LayerStrip
            layers={layersSnap}
            nodeId={node.node_id}
            outputId={outputId}
            reachable={node.reachable}
            onProblem={setProblem}
          />
        </div>

        <Panel width="var(--pm-inspector-width)" label="Armed">
          <PanelHeader
            title="Armed"
            icon={<Target size={16} aria-hidden color="var(--pm-state-armed)" />}
            trailing={
              armed ? (
                <StatusPill tone="armed" variant="subtle" size={10}>READY</StatusPill>
              ) : undefined
            }
          />
          <PanelBody pad16>
            {armed ? (
              <ArmedDetail
                item={armed}
                reachable={node.reachable}
                busy={busy !== null}
                problem={problem}
                takeLabel={busy === `take:${armed.id}` ? 'Taking…' : 'Take'}
                takeNote="Selecting an item arms it. Nothing appears until you take."
                onTake={() =>
                  // Minted HERE, at the press, and not regenerated on retry —
                  // that is what makes a double-click one take rather than two.
                  void fire(`take:${armed.id}`, {
                    cue_id: newCueId(),
                    node: node.node_id,
                    output: outputId,
                    layer: armed.destination?.layer ?? 'Lower Third',
                    op: 'take',
                    item: armed.id,
                    show: SHOW,
                  })
                }
                onAlso={(req) => void fire(`also:${armed.id}`, req)}
                onTakeOut={(req) => void fire(`out:${armed.id}`, req)}
                nodeId={node.node_id}
                outputId={outputId}
              />
            ) : (
              <p className="show-console__dim">Nothing yet. Build an item in Edit.</p>
            )}
            <TakeLog />
          </PanelBody>
        </Panel>
      </div>
    </div>
  )
}

/** A rack row with its own readiness ear, so staging shows where the eye is. */
function RackRow({
  item,
  armed,
  showing,
  onClick,
}: {
  item: Item
  armed: boolean
  showing: boolean
  onClick: () => void
}) {
  const readiness = useTopic<Readiness>(topics.itemReadiness(item.id))
  const staging = readiness ? !readiness.ready : false
  return (
    <RundownItem
      title={item.label}
      subtitle={item.destination?.layer ?? 'Lower Third'}
      state={showing ? 'air' : armed ? 'armed' : 'idle'}
      pill={
        showing
          ? { label: 'ON AIR', tone: 'air' }
          : armed
            ? { label: 'ARMED', tone: 'armed' }
            : undefined
      }
      dot={!showing && !armed ? (staging ? 'staged' : 'ready') : undefined}
      onClick={onClick}
    />
  )
}

// ---------------------------------------------------------------------------
// The armed inspector: the values about to be sent, next to what sends them.

function ArmedDetail({
  item,
  reachable,
  busy,
  problem,
  takeLabel,
  takeNote,
  onTake,
  onAlso,
  onTakeOut,
  nodeId,
  outputId,
}: {
  item: Item
  reachable: boolean
  busy: boolean
  problem: string | null
  takeLabel: string
  takeNote: string
  onTake: () => void
  onAlso: (req: CueRequest) => void
  onTakeOut: (req: CueRequest) => void
  nodeId: string
  outputId: string
}) {
  const readiness = useTopic<Readiness>(topics.itemReadiness(item.id))
  const layer = item.destination?.layer ?? 'Lower Third'

  // Not "warns" — NOT ARMABLE. An item whose node has not finished staging its
  // assets cannot be taken, and the button says why rather than failing later
  // (ADR 0006).
  //
  // `ready` means at least ONE destination is ready (ADR 0018). The take
  // works and does less than it looks like it will, so `partial` gets its own
  // line naming the destinations that will not fire. Said BEFORE the press:
  // afterwards it is just the ack reporting something that was knowable.
  const staging = readiness ? !readiness.ready : false
  const blocked = !reachable || staging
  const notFiring = (readiness?.destinations ?? []).filter((d) => !d.ready)
  // Destinations that wait to be asked for. Read from the Item rather than
  // from readiness, because whether one EXISTS is a property of the content
  // and whether it is ready is a property of the node.
  const onRequest = (item.destinations ?? []).filter((d) => d.mode === 'on_request')

  const fields = Object.entries(item.data ?? {}).slice(0, 6)

  return (
    <>
      <div className="show-console__armed-head">
        <GroupHeader label="Armed item" />
        <span className="show-console__armed-title">{item.label}</span>
      </div>

      {problem ? (
        <p className="show-console__problem" role="alert">
          <AlertTriangle size={15} aria-hidden /> {problem}
        </p>
      ) : null}

      {staging ? (
        // Two different sentences, because they are two different futures.
        // "Still copying" resolves itself by waiting; a template this node is
        // too old to read never will, and saying "still copying" about it
        // sends an operator to wait through something already decided.
        <p className="show-console__staging">
          {readiness?.unreadable
            ? `Cannot play here: ${readiness.unreadable}`
            : `still copying${readiness?.missing?.length ? ` · ${readiness.missing.length} file(s)` : ''}`}
        </p>
      ) : null}
      {readiness?.partial ? (
        <p className="show-console__partial">
          will not reach {notFiring.map((d) => `${d.node} ${d.output}/${d.layer}`).join(', ')}
        </p>
      ) : null}

      {fields.map(([k, v]) => (
        <FormGroup key={k} label={k} value={String(v)} />
      ))}

      {(item.destinations ?? []).length ? (
        <div className="show-console__dests">
          <GroupHeader label="Destinations" />
          {(item.destinations ?? []).map((d, n) => {
            const dr = readiness?.destinations?.find(
              (x) => x.output === d.output && x.layer === d.layer,
            )
            return (
              <div key={n} className="show-console__dest">
                <StatusDot tone={d.blank ? 'muted' : dr && !dr.ready ? 'staged' : 'ready'} />
                <span className="show-console__dest-route">
                  {d.output}  ·  {d.layer}
                </span>
                <span className="show-console__dest-note">
                  {d.blank
                    ? 'clears this feed'
                    : d.mode === 'on_request'
                      ? 'only when asked for'
                      : 'fires with the take'}
                </span>
              </div>
            )
          })}
        </div>
      ) : null}

      <div className="show-console__commit">
        <Button take disabled={blocked || busy} onClick={onTake}>
          {takeLabel}
        </Button>
        <span className="show-console__commit-note">{takeNote}</span>
      </div>

      <div className="show-console__secondary">
        {onRequest.length ? (
          // The second of the rundown's two actions: *"lower thirds always
          // have a full screen version we can take if we need to"*. Shown only
          // when this Item actually has one, so the panel does not grow a dead
          // button on every graphic.
          <Button
            variant="secondary"
            disabled={blocked || busy}
            title={`Also take to ${onRequest.map((d) => `${d.output}/${d.layer}`).join(', ')}`}
            onClick={() =>
              onAlso({
                cue_id: newCueId(),
                node: nodeId,
                output: outputId,
                layer,
                op: 'take_also',
                item: item.id,
                show: SHOW,
              })
            }
          >
            {busy ? 'Taking…' : 'Also full screen'}
          </Button>
        ) : null}
        <Button
          variant="secondary"
          disabled={!reachable || busy}
          onClick={() =>
            onTakeOut({
              cue_id: newCueId(),
              node: nodeId,
              output: outputId,
              layer,
              op: 'take_out',
              show: SHOW,
            })
          }
        >
          Take out
        </Button>
      </div>
    </>
  )
}

// ---------------------------------------------------------------------------
// Clear all, guarded. One press arms it, the second fires it — a mis-tap must
// not empty every layer, and a confirm dialog would be an overlay this
// console does not have. The guard disarms itself.

function ClearAll({
  nodeId,
  outputId,
  disabled,
  onProblem,
}: {
  nodeId: string
  outputId: string
  disabled: boolean
  onProblem: (p: string | null) => void
}) {
  const [primed, setPrimed] = useState(false)
  useEffect(() => {
    if (!primed) return
    const t = setTimeout(() => setPrimed(false), 3000)
    return () => clearTimeout(t)
  }, [primed])

  return (
    <Button
      variant={primed ? 'live' : 'secondary'}
      disabled={disabled}
      onClick={() => {
        if (!primed) {
          setPrimed(true)
          return
        }
        setPrimed(false)
        takeCue({
          cue_id: newCueId(),
          node: nodeId,
          output: outputId,
          op: 'clear_all',
          show: SHOW,
        }).catch((e) => onProblem(e instanceof ApiError ? e.message : String(e)))
      }}
    >
      {primed ? 'Press again to clear' : 'Clear all'}
    </Button>
  )
}

// ---------------------------------------------------------------------------
// The program well and the layer strip.

function ProgramWell({
  node,
  outputId,
  layers,
}: {
  node: NodeSummary
  outputId: string
  layers: LayersSnapshot | undefined
}) {
  const anyOnAir = (layers?.layers ?? []).some((l) => l.occupant)

  // Which output the MONITOR shows — not necessarily the one being operated.
  // A take fans out to the ITEM's destinations (ADR 0018), so a full-screen
  // fired from a lower-third rundown lands on another output; a monitor
  // pinned to one output makes that success look like a failure.
  const [watching, setWatching] = useState<string | null>(null)
  const outputs = node.state?.outputs ?? []
  const watched = watching ?? outputId
  const watchedPort = outputs.find((o) => o.id === watched)?.preview_port ?? 0
  // The node reports each output's own preview port; the advertised URL
  // carries the host. Port 0 means that output has no preview.
  const watchedUrl =
    node.preview_url && watchedPort > 0
      ? node.preview_url.replace(/:\d+\/?$/, `:${watchedPort}/`)
      : watched === outputId
        ? node.preview_url
        : null

  const caption = `${watched}${layers?.fps ? `  ·  ${layers.fps.toFixed(2)} fps` : ''}`

  return (
    <ProgramCanvas label="Program output" onAir={anyOnAir} output={caption}>
      {outputs.length > 1 ? (
        <div className="c-program__picker" role="tablist" aria-label="Watch output">
          {outputs.map((o) => (
            <button
              key={o.id}
              type="button"
              role="tab"
              aria-selected={o.id === watched}
              className={
                o.id === watched ? 'c-program__pick c-program__pick--on' : 'c-program__pick'
              }
              onClick={() => setWatching(o.id)}
            >
              {o.id}
            </button>
          ))}
        </div>
      ) : null}
      {watchedUrl ? (
        <Preview
          key={watchedUrl}
          url={watchedUrl}
          name={node.name}
          outputId={watched}
          anyOnAir={anyOnAir}
        />
      ) : (
        <div className="c-program__empty">
          {watched !== outputId
            ? `${watched} publishes no preview.`
            : anyOnAir
              ? `${node.name} publishes no preview. What is on air is listed below.`
              : 'Nothing on air, and no preview from this node.'}
        </div>
      )}
    </ProgramCanvas>
  )
}

/**
 * The layer strip: what the NODE says each layer holds, one card per layer.
 * Top of the stack first — the wire order is compositing order, bottom-first,
 * but an operator reads a stack the way they see it.
 */
function LayerStrip({
  layers,
  nodeId,
  outputId,
  reachable,
  onProblem,
}: {
  layers: LayersSnapshot | undefined
  nodeId: string
  outputId: string
  reachable: boolean
  onProblem: (p: string | null) => void
}) {
  if (!layers?.layers?.length) {
    return (
      <section className="show-console__layers" aria-label="On air">
        <p className="show-console__dim">Nothing on air.</p>
      </section>
    )
  }
  return (
    <section className="show-console__layers" aria-label="On air">
      {layers.layers.toReversed().map((l) => (
        <LayerCard
          key={l.name}
          layer={l.name}
          state={l.occupant ? 'ON AIR' : 'CLEAR'}
          tone={l.occupant ? 'air' : 'muted'}
          content={
            l.occupant
              ? // Present tense, and the graphic's own words — not an id. A
                // graphic that could not be drawn as authored says so: it is on
                // air AND it is wrong, and saying nothing would be the failure.
                `${l.occupant.item_label ?? l.occupant.item_id} · ${l.occupant.phase}${l.occupant.degraded ? ' · degraded' : ''}`
              : 'Nothing on this layer'
          }
          action={l.occupant ? 'Take out' : undefined}
          actionDisabled={!reachable}
          onAction={() =>
            takeCue({
              cue_id: newCueId(),
              node: nodeId,
              output: outputId,
              layer: l.name,
              op: 'take_out',
              show: SHOW,
            }).catch((e) => onProblem(e instanceof ApiError ? e.message : String(e)))
          }
        >
          {l.occupant?.timelines?.length ? (
            <TimelineCues
              occupant={l.occupant}
              nodeId={nodeId}
              outputId={outputId}
              layer={l.name}
              reachable={reachable}
              onProblem={onProblem}
            />
          ) : null}
        </LayerCard>
      ))}
    </section>
  )
}

/**
 * The named-timeline cues the ON-AIR graphic offers — `Update`, `Highlight`,
 * whatever the designer called them. `In` and `Out` never appear here: the
 * take buttons own the lifecycle, and these are flourishes layered on top of
 * it. The list comes from the pinned revision's manifest, so the buttons
 * describe the document actually on air, not the template's newest draft.
 *
 * A chip whose run is RUNNING lights up — the node reports that, so the light
 * is the graphic moving, not the button hoping. Clicking a running chip stops
 * it, which is the toggle a looping shimmer needs; clicking an idle one plays
 * it (again) from the top.
 */
function TimelineCues({
  occupant,
  nodeId,
  outputId,
  layer,
  reachable,
  onProblem,
}: {
  occupant: Occupant
  nodeId: string
  outputId: string
  layer: string
  reachable: boolean
  onProblem: (p: string | null) => void
}) {
  const running = new Set(occupant.named ?? [])
  return (
    <div className="show-console__cues" aria-label="Timeline cues">
      {(occupant.timelines ?? []).map((name) => {
        const on = running.has(name)
        return (
          <button
            key={name}
            type="button"
            className={on ? 'show-console__cue show-console__cue--running' : 'show-console__cue'}
            disabled={!reachable}
            title={on ? `Stop ${name}` : `Play ${name}`}
            onClick={() =>
              takeCue({
                cue_id: newCueId(),
                node: nodeId,
                output: outputId,
                layer,
                op: 'timeline',
                timeline: name,
                action: on ? 'stop' : 'play',
                show: SHOW,
              }).catch((e) => onProblem(e instanceof ApiError ? e.message : String(e)))
            }
          >
            {name}
          </button>
        )
      })}
    </div>
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
    <section className="show-console__log" aria-label="Take log">
      <GroupHeader label="Take log" />
      {entries.length === 0 ? (
        <p className="show-console__dim">Nothing has been taken yet.</p>
      ) : (
        <div className="show-console__log-list">
          {entries.map((e) => (
            <LogRow
              key={e.seq}
              entry={[
                new Date(e.at_utc).toLocaleTimeString(undefined, { hour12: false }),
                e.op.replace('_', ' '),
                e.item_label ?? e.layer ?? '',
                // Which surface fired it. Without this the log is baffling the
                // first time someone uses a GPI or Companion (ADR 0014).
                e.surface.replace('_', ' '),
              ]
                .filter(Boolean)
                .join('  ·  ')}
              result={
                e.result === 'applied'
                  ? e.op === 'take' || e.op === 'take_also'
                    ? 'LIVE'
                    : 'CLEAR'
                  : e.result.toUpperCase()
              }
            />
          ))}
        </div>
      )}
    </section>
  )
}

/**
 * The preview canvas, fed by WebCodecs.
 *
 * The checkerboard is drawn INTO the frame by the node, because the output is a
 * key signal and a black rectangle could be a graphic or could be nothing.
 *
 * Every failure says which one it is. "No preview" and "no graphic" need
 * completely different responses from an operator, and a blank box says both.
 */
function Preview({
  url,
  name,
  outputId,
  anyOnAir,
}: {
  url: string
  name: string
  outputId: string
  anyOnAir: boolean
}) {
  const canvasRef = useRef<HTMLCanvasElement>(null)
  const [status, setStatus] = useState<PreviewStatus>({ state: 'connecting' })
  const [levels, setLevels] = useState<number[]>([])
  // Peak-hold per channel: the loudest recent moment, decaying — the mark an
  // operator's eye actually reads on a meter.
  const holds = useRef<{ db: number; at: number }[]>([])

  useEffect(() => {
    const canvas = canvasRef.current
    if (!canvas) return
    const handle = startH264Preview(previewSocketUrl(url), canvas, setStatus, (dbfs) => {
      const now = performance.now()
      dbfs.forEach((db, i) => {
        const h = holds.current[i]
        // Hold 1.5 s, then fall.
        if (!h || db >= h.db || now - h.at > 1500) holds.current[i] = { db, at: now }
      })
      setLevels(dbfs)
    })
    return () => handle.close()
  }, [url])

  // Channels split by PAIR: 1/2 ride the left edge, 3/4 the right — the
  // layout an operator expects on a quad-channel feed. A stereo output only
  // grows the left pair.
  const left = levels.slice(0, 2)
  const right = levels.slice(2, 4)

  return (
    <>
      <canvas
        ref={canvasRef}
        className="c-program__frame"
        aria-label={`Preview of ${outputId}`}
        hidden={status.state !== 'live'}
      />
      {status.state === 'live' && levels.length > 0 ? (
        <>
          <Meters side="left" dbfs={left} holds={holds.current.slice(0, 2)} firstChannel={1} />
          {right.length > 0 ? (
            <Meters side="right" dbfs={right} holds={holds.current.slice(2, 4)} firstChannel={3} />
          ) : null}
        </>
      ) : null}
      {status.state !== 'live' ? (
        <div className="c-program__empty">
          {status.state === 'connecting' || status.state === 'waiting-for-key'
            ? `Connecting to ${name}’s preview…`
            : `No preview from ${name}. ${status.detail} What’s on air is unaffected.${anyOnAir ? ' See layers below.' : ''}`}
        </div>
      ) : null}
    </>
  )
}

/** dBFS → 0..1 of the bar, floored at −60: the range a programme meter reads. */
const meterFrac = (db: number) => Math.max(0, Math.min(1, (db + 60) / 60))

/**
 * Peak meters overlaying one edge of the program monitor.
 *
 * The zone colours are the meter convention every console uses — green body,
 * amber above −18, red only at the top −6 — and deliberately NOT the console's
 * status palette: a meter's red means "about to clip", which is an audio fact,
 * not an ON AIR mark (ADR 0020 governs status colour, and a meter is an
 * instrument).
 */
function Meters({
  side,
  dbfs,
  holds,
  firstChannel,
}: {
  side: 'left' | 'right'
  dbfs: number[]
  holds: { db: number; at: number }[]
  firstChannel: number
}) {
  return (
    <div className={`c-meters c-meters--${side}`} aria-label={`Audio channels ${firstChannel}–${firstChannel + dbfs.length - 1}`}>
      {dbfs.map((db, i) => (
        <div key={i} className="c-meter" title={`Channel ${firstChannel + i}: ${db <= -119 ? 'silent' : `${db.toFixed(1)} dBFS`}`}>
          {/* clip-path, not scaleY: the fill carries the zone gradient at full
              height, and scaling it would drag the red zone down into quiet
              levels. Clipping keeps every zone at its own dB. */}
          <div
            className="c-meter__fill"
            style={{ clipPath: `inset(${(1 - meterFrac(db)) * 100}% 0 0 0)` }}
          />
          {holds[i] && holds[i].db > -119 ? (
            <div className="c-meter__peak" style={{ bottom: `${meterFrac(holds[i].db) * 100}%` }} />
          ) : null}
        </div>
      ))}
    </div>
  )
}
