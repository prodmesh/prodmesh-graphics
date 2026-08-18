/**
 * Configure — for whoever set up the room.
 *
 * Node discovery and adoption, outputs, layer stacks, colour, and test
 * patterns, in the console language (ADR 0020).
 *
 * # Adoption is a real boundary, not a formality
 *
 * A node that dials this server is welcomed and then given **nothing** until
 * someone adopts it. On a shared church network the thing that must not quietly
 * start receiving this week's graphics is a laptop somebody left running a
 * stale build — so an unadopted node appears here as a stranger asking to join,
 * and cues to it are refused with a sentence pointing back at this mode.
 *
 * # Discovery is an aid, never an authority
 *
 * The server advertises over mDNS so a node can find it with nothing typed in.
 * Multicast is unreliable across VLANs and behind switches with poor IGMP
 * snooping, so a node can always be pointed at an address by hand — and this
 * mode says so rather than presenting discovery as the only way in.
 */

import { useCallback, useState } from 'react'
import { Check, ChevronDown, ChevronUp, Plus, X } from 'lucide-react'
import { useQuery, invalidate } from '../../lib/useQuery'
import { keys } from '../../lib/keys'
import { ApiError, post, put } from '../../lib/api'
import {
  Button,
  FormGroup,
  GroupHeader,
  NodeCard,
  StatusPill,
  Tag,
} from '../../components/console'
import type { NodeSummary } from '../show/types'

type ColorConfig = {
  matrix: string
  range: string
  key_range: string
  key_mode: string
}

/** What each pattern proves (`docs/specs/color-and-key.md` §7). */
const PATTERNS: { id: string; label: string; proves: string }[] = [
  { id: 'ramp', label: 'Marked ramp', proves: 'range — the marks land at 16 and 235 narrow, 0 and 255 full' },
  { id: 'bars', label: '75% colour bars', proves: 'matrix and primaries — vectorscope boxes' },
  { id: 'softedge', label: 'Soft-edged white', proves: 'premultiplication — key it over bright AND dark; any fringe is a bug' },
  { id: 'alpha', label: 'Alpha staircase', proves: 'key range and linearity' },
  { id: 'line', label: 'Moving 1px line', proves: 'frame timing — tearing or stutter is a late frame' },
]

type Capabilities = {
  outputs?: { device: string; kind: string; supports_key_fill: boolean; has_genlock: boolean }[]
  gpu?: { adapter: string; backend: string } | null
  max_layers?: number
  scene_reader_major?: number
}

type FullNode = NodeSummary & { adopted: boolean; capabilities: Capabilities | null }

export function Configure() {
  const { data } = useQuery<{ nodes: FullNode[] }>(keys.nodes())
  const nodes = data?.nodes ?? []
  const adopted = nodes.filter((n) => n.adopted)
  const strangers = nodes.filter((n) => !n.adopted)

  return (
    <div className="cfg-console">
      <div className="console__toolbar">
        <div className="console__toolbar-group">
          <Tag pill tone="raised">Playout nodes</Tag>
          <span className="console__toolbar-note">
            A node finds this server on the network by itself, and can always be given an address
            by hand — --server &lt;host:port&gt;.
          </span>
        </div>
        <Tag size={10}>{`${adopted.length} adopted`}</Tag>
      </div>

      <div className="mode-scroll cfg-console__scroll">
        {strangers.length > 0 ? (
          <section className="cfg-console__section" aria-label="New nodes">
            <GroupHeader label="New nodes" trailing={`${strangers.length}`} />
            {strangers.map((n) => (
              <Stranger key={n.node_id} node={n} />
            ))}
          </section>
        ) : null}

        <section className="cfg-console__section" aria-label="Playout nodes">
          <GroupHeader label="Playout nodes" />
          {adopted.length === 0 ? (
            <div className="mode__stub">
              <p>No playout node has been adopted yet.</p>
              <p>
                Start a node and point it at this server. It finds this server on the network by
                itself, and you can always give it an address by hand —{' '}
                <code>--server &lt;host:port&gt;</code>.
              </p>
            </div>
          ) : (
            adopted.map((n) => <AdoptedNode key={n.node_id} node={n} />)
          )}
        </section>
      </div>
    </div>
  )
}

function Stranger({ node }: { node: FullNode }) {
  const [busy, setBusy] = useState(false)
  const adopt = useCallback(async () => {
    setBusy(true)
    try {
      await post(`${keys.node(node.node_id)}/adopt`, { adopted: true })
      invalidate(keys.nodes())
    } finally {
      setBusy(false)
    }
  }, [node.node_id])

  return (
    <div className="cfg-console__stranger">
      <div className="cfg-console__stranger-text">
        <span className="cfg-console__stranger-name">
          {node.name}
          <span className="cfg-console__mono">
            {'  ·  '}{node.node_id.slice(0, 10)}…{'  ·  '}v{node.version ?? '?'}
          </span>
        </span>
        <span className="cfg-console__dim">
          This node is asking to join. Until it is adopted it receives no templates, no items,
          and no cues.
        </span>
      </div>
      <Button onClick={() => void adopt()} disabled={busy}>
        <Check size={15} aria-hidden /> {busy ? 'Adopting…' : 'Adopt'}
      </Button>
    </div>
  )
}

function AdoptedNode({ node }: { node: FullNode }) {
  const { data } = useQuery<{
    outputs: { output_id: string; mode: string; layers: string[]; color?: ColorConfig }[]
  }>(`${keys.node(node.node_id)}/outputs`)
  const caps = node.capabilities
  const outputs = data?.outputs ?? []
  const liveOutputs = node.state?.outputs ?? []

  return (
    <div className="cfg-console__node">
      <NodeCard
        name={node.name}
        role={node.reachable ? undefined : 'OFFLINE'}
        healthy={node.reachable}
        status={
          node.reachable
            ? undefined
            : `${node.name} is offline. Anything already on air is still on air.`
        }
        metrics={[
          { label: 'Version', value: `v${node.version ?? '?'}` },
          { label: 'GPU', value: caps?.gpu ? `${caps.gpu.adapter} (${caps.gpu.backend})` : 'not reported' },
          {
            label: 'Devices',
            value: caps?.outputs?.length
              ? caps.outputs.map((o) => `${o.device}${o.supports_key_fill ? ' · key+fill' : ''}`).join(', ')
              : 'none — NDI has no enumerable hardware',
          },
          { label: 'Max layers', value: String(caps?.max_layers ?? '?'), accent: true },
        ]}
      />

      {liveOutputs.some((o) => o.sync) ? (
        <div className="cfg-console__refs">
          {liveOutputs.filter((o) => o.sync).map((o) => (
            <div key={o.id} className="cfg-console__ref">
              <span className="cfg-console__mono">Reference{'  ·  '}{o.id}</span>
              {o.sync?.locked ? (
                <StatusPill tone="ready" variant="subtle" size={10}>
                  {`LOCKED · ${(o.sync.format || 'unknown format').toUpperCase()}`}
                </StatusPill>
              ) : o.sync?.present ? (
                // Present but not locked is the interesting state: a reference
                // is connected and the card is not following it, which usually
                // means a format mismatch.
                <StatusPill tone="staged" variant="subtle" size={10}>
                  {`PRESENT, NOT LOCKED${o.sync.format ? ` · ${o.sync.format.toUpperCase()}` : ''}`}
                </StatusPill>
              ) : (
                <StatusPill tone="muted" variant="subtle" size={10}>
                  NO REFERENCE — FREE RUNNING
                </StatusPill>
              )}
            </div>
          ))}
        </div>
      ) : null}

      {(outputs.length
        ? outputs
        : liveOutputs.map((o) => ({ output_id: o.id, mode: '', layers: [], color: undefined }))
      ).map((o) => (
        <OutputEditor
          key={o.output_id}
          nodeId={node.node_id}
          outputId={o.output_id}
          mode={o.mode}
          color={o.color}
          layers={o.layers.length ? o.layers : (liveOutputs.find((l) => l.id === o.output_id)?.layers ?? []).map((l) => l.name)}
        />
      ))}
    </div>
  )
}

/**
 * The layer stack, bottom first.
 *
 * That order **is** the compositing order, so the arrows are not cosmetic —
 * moving a layer up puts it in front of the one it passes. Layers are addressed
 * by name everywhere else in the system, so renaming one here would orphan
 * every Item pointing at it; there is deliberately no rename.
 */
function OutputEditor({
  nodeId,
  outputId,
  mode,
  layers,
  color,
}: {
  nodeId: string
  outputId: string
  mode: string
  layers: string[]
  color?: ColorConfig
}) {
  const [draft, setDraft] = useState<string[]>(layers)
  const [col, setCol] = useState<ColorConfig>(
    color ?? { matrix: 'bt709', range: 'narrow', key_range: 'narrow', key_mode: 'fill' },
  )
  const [pattern, setPattern] = useState<string | null>(null)
  const [adding, setAdding] = useState('')
  const [problem, setProblem] = useState<string | null>(null)
  const [saved, setSaved] = useState(false)

  const save = useCallback(
    async (next: string[]) => {
      setProblem(null)
      try {
        await put(`${keys.node(nodeId)}/outputs/${outputId}`, {
          layers: next,
          mode: mode || '1920x1080',
          color: col,
        })
        setDraft(next)
        setSaved(true)
        invalidate(`${keys.node(nodeId)}/outputs`)
        invalidate(keys.nodes())
        setTimeout(() => setSaved(false), 1500)
      } catch (e) {
        setProblem(e instanceof ApiError ? e.message : String(e))
      }
    },
    [nodeId, outputId, mode, col],
  )

  const move = (i: number, delta: number) => {
    const to = i + delta
    if (to < 0 || to >= draft.length) return
    const next = [...draft]
    const [x] = next.splice(i, 1)
    next.splice(to, 0, x as string)
    void save(next)
  }

  return (
    <div className="cfg-console__output">
      <div className="cfg-console__output-head">
        <span className="cfg-console__output-name">
          {outputId} <span className="cfg-console__mono">{mode || '1920x1080'}</span>
        </span>
        {saved ? (
          <StatusPill tone="ready" variant="subtle" size={10}>PUSHED TO THE NODE</StatusPill>
        ) : null}
      </div>
      <p className="cfg-console__dim">
        Bottom of the stack first. What is lower is covered by what is above it.
      </p>

      {problem ? (
        <p className="cfg-console__problem" role="alert">{problem}</p>
      ) : null}

      <div className="cfg-console__layers">
        {draft.map((name, i) => (
          <div key={name} className="cfg-console__layer">
            <span className="cfg-console__mono">{String(i + 1).padStart(2, '0')}</span>
            <span className="cfg-console__layer-name">{name}</span>
            <button
              type="button"
              className="edit-console__iconbtn"
              aria-label={`Move ${name} down the stack`}
              disabled={i === 0}
              onClick={() => move(i, -1)}
            >
              <ChevronDown size={13} aria-hidden /> back
            </button>
            <button
              type="button"
              className="edit-console__iconbtn"
              aria-label={`Move ${name} up the stack`}
              disabled={i === draft.length - 1}
              onClick={() => move(i, 1)}
            >
              <ChevronUp size={13} aria-hidden /> front
            </button>
            <button
              type="button"
              className="edit-console__iconbtn"
              aria-label={`Remove ${name}`}
              onClick={() => void save(draft.filter((l) => l !== name))}
            >
              <X size={13} aria-hidden />
            </button>
          </div>
        ))}
      </div>

      <div className="cfg-console__grid">
        <div className="cfg-console__fieldset">
          <GroupHeader label="Colour" />
          {([
            ['matrix', 'Matrix', ['bt709', 'bt601', 'bt2020']],
            ['range', 'Fill range', ['narrow', 'full']],
            // Separate from the fill range on purpose: a narrow fill with a
            // full key is a real plant configuration.
            ['key_range', 'Key range', ['narrow', 'full']],
            ['key_mode', 'Key mode', ['fill', 'key', 'fill_key', 'embedded']],
          ] as const).map(([field, label, options]) => (
            <FormGroup key={field} label={label}>
              <select
                value={col[field]}
                onChange={(e) => {
                  const next = { ...col, [field]: e.target.value }
                  setCol(next)
                  void save(draft)
                }}
              >
                {options.map((o) => (
                  <option key={o} value={o}>{o}</option>
                ))}
              </select>
            </FormGroup>
          ))}
        </div>

        <div className="cfg-console__fieldset">
          <GroupHeader label="Test patterns" />
          <p className="cfg-console__dim">
            These sit above the layer stack, so you can prove a configuration without clearing
            the show off air.
          </p>
          <div className="cfg-console__patterns">
            {PATTERNS.map((p) => (
              <Button
                key={p.id}
                variant={pattern === p.id ? 'primary' : 'secondary'}
                title={`Proves ${p.proves}`}
                onClick={() => {
                  const next = pattern === p.id ? null : p.id
                  setPattern(next)
                  void post(`${keys.node(nodeId)}/outputs/${outputId}/test-pattern`, { pattern: next })
                }}
              >
                {p.label}
              </Button>
            ))}
          </div>
          {pattern ? (
            <p className="cfg-console__dim">{PATTERNS.find((p) => p.id === pattern)?.proves}</p>
          ) : null}
        </div>
      </div>

      <form
        className="cfg-console__add"
        onSubmit={(e) => {
          e.preventDefault()
          const name = adding.trim()
          if (!name) return
          setAdding('')
          void save([...draft, name])
        }}
      >
        <input
          value={adding}
          onChange={(e) => setAdding(e.target.value)}
          placeholder="New layer name"
          aria-label="New layer name"
        />
        <button type="submit" className="c-btn c-btn--secondary">
          <Plus size={13} aria-hidden /> Add
        </button>
      </form>
    </div>
  )
}
