/**
 * Configure — for whoever set up the room.
 *
 * Node discovery and adoption, outputs, and layer stacks. Formats, colour, key
 * mode, reference status and test patterns are M3.
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
import { Check, Cpu, Layers, MonitorPlay, Plus, X } from 'lucide-react'
import { useQuery, invalidate } from '../../lib/useQuery'
import { keys } from '../../lib/keys'
import { ApiError, post, put } from '../../lib/api'
import type { NodeSummary } from '../show/types'

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
    <>
      <h1>Configure</h1>

      {strangers.length > 0 ? (
        <section className="cfg__strangers" aria-label="New nodes">
          <h2>New nodes</h2>
          {strangers.map((n) => (
            <Stranger key={n.node_id} node={n} />
          ))}
        </section>
      ) : null}

      <section aria-label="Playout nodes">
        <h2 className="cfg__h2">Playout nodes</h2>
        {adopted.length === 0 ? (
          <div className="mode__stub">
            <p>No playout node has been adopted yet.</p>
            <p>
              Start a node and point it at this server. It finds this server on
              the network by itself, and you can always give it an address by
              hand — <code>--server &lt;host:port&gt;</code>.
            </p>
          </div>
        ) : (
          adopted.map((n) => <NodeCard key={n.node_id} node={n} />)
        )}
      </section>
    </>
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
    <div className="cfg__stranger">
      <div>
        <strong>{node.name}</strong>
        <span className="show__dim"> · {node.node_id.slice(0, 10)}… · v{node.version ?? '?'}</span>
        <p className="show__dim">
          This node is asking to join. Until it is adopted it receives no
          templates, no items, and no cues.
        </p>
      </div>
      <button type="button" className="btn btn--take" onClick={() => void adopt()} disabled={busy}>
        <Check size={15} aria-hidden /> {busy ? 'Adopting…' : 'Adopt'}
      </button>
    </div>
  )
}

function NodeCard({ node }: { node: FullNode }) {
  const { data } = useQuery<{ outputs: { output_id: string; mode: string; layers: string[] }[] }>(
    `${keys.node(node.node_id)}/outputs`,
  )
  const caps = node.capabilities
  const outputs = data?.outputs ?? []
  const liveOutputs = node.state?.outputs ?? []

  return (
    <div className="cfg__node">
      <header>
        <span className={node.reachable ? 'cfg__dot cfg__dot--up' : 'cfg__dot'} aria-hidden />
        <strong>{node.name}</strong>
        <span className="show__dim">v{node.version ?? '?'}</span>
        {!node.reachable ? <span className="show__dim">offline</span> : null}
      </header>

      <dl className="cfg__caps">
        <div><dt><Cpu size={13} aria-hidden /> GPU</dt><dd>{caps?.gpu ? `${caps.gpu.adapter} (${caps.gpu.backend})` : 'not reported'}</dd></div>
        <div><dt><MonitorPlay size={13} aria-hidden /> Devices</dt>
          <dd>
            {caps?.outputs?.length
              ? caps.outputs.map((o) => `${o.device}${o.supports_key_fill ? ' · key+fill' : ''}`).join(', ')
              : 'none reported — NDI has no enumerable hardware'}
          </dd>
        </div>
        <div><dt><Layers size={13} aria-hidden /> Max layers</dt><dd>{caps?.max_layers ?? '?'}</dd></div>
      </dl>

      {(outputs.length ? outputs : liveOutputs.map((o) => ({ output_id: o.id, mode: '', layers: [] }))).map((o) => (
        <OutputEditor
          key={o.output_id}
          nodeId={node.node_id}
          outputId={o.output_id}
          mode={o.mode}
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
}: {
  nodeId: string
  outputId: string
  mode: string
  layers: string[]
}) {
  const [draft, setDraft] = useState<string[]>(layers)
  const [adding, setAdding] = useState('')
  const [problem, setProblem] = useState<string | null>(null)
  const [saved, setSaved] = useState(false)

  const save = useCallback(
    async (next: string[]) => {
      setProblem(null)
      try {
        await put(`${keys.node(nodeId)}/outputs/${outputId}`, { layers: next, mode: mode || '1920x1080' })
        setDraft(next)
        setSaved(true)
        invalidate(`${keys.node(nodeId)}/outputs`)
        invalidate(keys.nodes())
        setTimeout(() => setSaved(false), 1500)
      } catch (e) {
        setProblem(e instanceof ApiError ? e.message : String(e))
      }
    },
    [nodeId, outputId, mode],
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
    <div className="cfg__output">
      <h4>
        {outputId} <span className="show__dim">{mode || '1920x1080'}</span>
        {saved ? <span className="cfg__saved"> · pushed to the node</span> : null}
      </h4>
      <p className="show__dim">Bottom of the stack first. What is lower is covered by what is above it.</p>

      {problem ? <p className="show__problem" role="alert">{problem}</p> : null}

      <ol className="cfg__layers">
        {draft.map((name, i) => (
          <li key={name}>
            <span className="running__pos">{i + 1}</span>
            <span className="running__label">{name}</span>
            <button type="button" className="btn btn--ghost btn--small" aria-label={`Move ${name} down the stack`}
                    disabled={i === 0} onClick={() => move(i, -1)}>↓ back</button>
            <button type="button" className="btn btn--ghost btn--small" aria-label={`Move ${name} up the stack`}
                    disabled={i === draft.length - 1} onClick={() => move(i, 1)}>↑ front</button>
            <button type="button" className="btn btn--ghost btn--small" aria-label={`Remove ${name}`}
                    onClick={() => void save(draft.filter((l) => l !== name))}>
              <X size={13} aria-hidden />
            </button>
          </li>
        ))}
      </ol>

      <form
        className="cfg__add"
        onSubmit={(e) => {
          e.preventDefault()
          const name = adding.trim()
          if (!name) return
          setAdding('')
          void save([...draft, name])
        }}
      >
        <input value={adding} onChange={(e) => setAdding(e.target.value)} placeholder="New layer name" aria-label="New layer name" />
        <button type="submit" className="btn btn--ghost btn--small"><Plus size={13} aria-hidden /> Add</button>
      </form>
    </div>
  )
}
