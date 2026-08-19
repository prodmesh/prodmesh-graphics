/**
 * The Media Browser — every asset this server holds, and where it actually is.
 *
 * The second half is the point. "Uploaded" is a fact about the server;
 * whether Saturday's graphics render is a fact about the NODES, and the gap
 * between those two is exactly the failure that gets found at 10:15 on
 * Sunday. This dialog puts the gap on screen: per asset, per adopted node —
 * on it, still copying, missing, or not needed.
 *
 * # Where the answer comes from
 *
 * Nothing new was invented to know this. The assignment already names every
 * hash a node must hold, the node's readiness report already names what is
 * still outstanding, and `SyncProgress` already carries the transfer
 * aggregate — the server derives possession from those (`/api/assets/sync`).
 * A node that has not reported against the current assignment shows "no
 * report", never a green chip: "we have not heard" and "nothing is missing"
 * are different statements.
 *
 * # Names are the files' own
 *
 * A font row is named by what the file says it is — parsed by the same
 * engine that shapes with it — because that is the name the picker offers
 * and the declaration carries. Images show themselves.
 */

import { useEffect } from 'react'
import { X } from 'lucide-react'
import { invalidate, useQuery } from '../../lib/useQuery'
import { keys } from '../../lib/keys'
import { GroupHeader, StatusPill, Tag } from '../../components/console'
import type { FaceInfo } from './useDesign'

type SyncNode = {
  node_id: string
  name: string
  reachable: boolean
  /** True when the node has reported readiness against the CURRENT assignment. */
  reported: boolean
  required: string[]
  /** Hashes still outstanding — null until the node has reported. */
  missing: string[] | null
  staging: { total_bytes: number; done_bytes: number; pending: number } | null
}

type SyncAnswer = {
  assets: { hash: string; size: number; kind: string; first_seen: number }[]
  used_by: Record<string, string[]>
  nodes: SyncNode[]
}

const shortHash = (hash: string) => hash.replace(/^[a-z0-9]+:/, '').slice(0, 8)

const kb = (size: number) => `${Math.max(1, Math.round(size / 1024))} KB`

/** One node's relationship to one asset, as a chip. */
function NodeChip({ node, hash }: { node: SyncNode; hash: string }) {
  if (!node.required.includes(hash)) {
    return <Tag size={10}>not needed</Tag>
  }
  if (node.missing === null) {
    return (
      <StatusPill tone="muted" variant="subtle" size={10}>
        {node.reachable ? 'NO REPORT YET' : 'UNREACHABLE'}
      </StatusPill>
    )
  }
  if (node.missing.includes(hash)) {
    return (
      <StatusPill tone="staged" size={10}>
        {node.staging && node.staging.pending > 0 ? 'COPYING' : 'MISSING'}
      </StatusPill>
    )
  }
  return <StatusPill tone="ready" size={10}>ON {node.name.toUpperCase()}</StatusPill>
}

export function MediaBrowser({
  faces,
  onClose,
}: {
  /** Parsed font identities, so a font row carries its file's own name. */
  faces: FaceInfo[]
  onClose: () => void
}) {
  const { data } = useQuery<SyncAnswer>(keys.assetSync())

  // Possession changes while the dialog is open — that is what it is FOR — so
  // it refreshes itself while mounted. Five seconds matches the node report
  // cadence; anything faster polls the same answer.
  useEffect(() => {
    const t = setInterval(() => invalidate(keys.assetSync()), 5000)
    return () => clearInterval(t)
  }, [])

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => { if (e.key === 'Escape') onClose() }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [onClose])

  const assets = data?.assets ?? []
  const nodes = data?.nodes ?? []
  const fonts = assets.filter((a) => a.kind === 'font')
  const images = assets.filter((a) => a.kind === 'image')
  const other = assets.filter((a) => a.kind !== 'font' && a.kind !== 'image')

  const row = (a: { hash: string; size: number }, name: React.ReactNode, preview?: React.ReactNode) => (
    <div key={a.hash} className="mdb__row">
      {preview ?? <span className="mdb__swatch" aria-hidden />}
      <div className="mdb__name">
        <span>{name}</span>
        <span className="mdb__meta">
          {shortHash(a.hash)} · {kb(a.size)}
          {(data?.used_by[a.hash] ?? []).length
            ? ` · used by ${data!.used_by[a.hash]!.join(', ')}`
            : ' · not used by any published template'}
        </span>
      </div>
      <div className="mdb__chips">
        {nodes.length === 0 ? (
          <Tag size={10}>no nodes adopted</Tag>
        ) : (
          nodes.map((n) => <NodeChip key={n.node_id} node={n} hash={a.hash} />)
        )}
      </div>
    </div>
  )

  return (
    <div className="mdb" role="dialog" aria-modal="true" aria-label="Media Browser" onClick={onClose}>
      <div className="mdb__panel" onClick={(e) => e.stopPropagation()}>
        <header className="mdb__header">
          <span className="mdb__title">Media Browser</span>
          <button type="button" className="edit-console__iconbtn" aria-label="Close" onClick={onClose}>
            <X size={15} aria-hidden />
          </button>
        </header>

        <div className="mdb__body">
          {nodes.some((n) => n.staging && n.staging.pending > 0) ? (
            <p className="mdb__copying" role="status">
              {nodes
                .filter((n) => n.staging && n.staging.pending > 0)
                .map((n) =>
                  `Still copying to ${n.name} — ${n.staging!.pending} file(s), ` +
                  `${Math.max(0, Math.round((n.staging!.total_bytes - n.staging!.done_bytes) / (1024 * 1024)))} MB left`)
                .join(' · ')}
            </p>
          ) : null}

          <GroupHeader label="Fonts" trailing={fonts.length ? `${fonts.length}` : undefined} />
          {fonts.length === 0 ? <p className="mdb__dim">No fonts uploaded yet.</p> : null}
          {fonts.map((a) => {
            const face = faces.find((f) => f.hash === a.hash)
            return row(
              a,
              face ? `${face.family} · ${face.weight}` : 'Unreadable font file',
            )
          })}

          <GroupHeader label="Images" trailing={images.length ? `${images.length}` : undefined} />
          {images.length === 0 ? <p className="mdb__dim">No images uploaded yet.</p> : null}
          {images.map((a) =>
            row(a, shortHash(a.hash), <img className="mdb__thumb" src={`/cas/${a.hash}`} alt="" />),
          )}

          {other.length ? (
            <>
              <GroupHeader label="Other" trailing={`${other.length}`} />
              {other.map((a) => row(a, shortHash(a.hash)))}
            </>
          ) : null}
        </div>

        <footer className="mdb__footer">
          <span className="mdb__dim">
            A chip per adopted node. An asset is required only where an assignment references
            it; “not needed” means no Item on that node uses it yet.
          </span>
        </footer>
      </div>
    </div>
  )
}
