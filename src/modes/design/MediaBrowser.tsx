/**
 * The Media Browser — every asset this server holds, where it actually is,
 * and THE place media enters the system.
 *
 * Uploads live here, all of them: fonts and images go straight to the CAS,
 * and videos go through INGEST — the server probes the file and transcodes
 * long-GOP sources to an all-intra intermediate, because long-GOP H.264 is
 * where frame-accurate seek goes to die. An ingest is a JOB with states worth
 * watching, so running jobs render as rows until they land.
 *
 * # The sync half
 *
 * "Uploaded" is a fact about the server; whether Saturday's graphics render
 * is a fact about the NODES, and the gap between those two is exactly the
 * failure that gets found at 10:15 on Sunday. Per asset, per adopted node:
 * on it, still copying, missing, or not needed — derived entirely from what
 * nodes already report (`/api/assets/sync`). A node that has not reported
 * against the current assignment shows "no report", never a green chip.
 *
 * # Names are the files' own, and then they are the operator's
 *
 * An asset arrives with a perfectly good name and the CAS throws it away: the
 * hash is the identity, and `4a3b0d7b` is a terrible label to pick a plate by.
 * So an upload keeps its filename, and every row's name is editable here —
 * this is the one place that names things, and every picker reads it.
 *
 * A font is the exception and stays named by what the FILE says it is, parsed
 * by the same engine that shapes with it. Renaming "Inter · 700" to something
 * else would only disagree with the family a template actually declares.
 */

import { useEffect, useRef, useState } from 'react'
import { Clapperboard, FileUp, X } from 'lucide-react'
import { invalidate, useQuery } from '../../lib/useQuery'
import { keys } from '../../lib/keys'
import { patch, postBytes } from '../../lib/api'
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

type AssetRow = {
  hash: string
  size: number
  kind: string
  /** What a person calls it. Null for anything uploaded before names. */
  name?: string | null
  media?: {
    codec?: string
    width?: number
    height?: number
    fps?: number
    duration_s?: number
    filename?: string
  } | null
}

type SyncAnswer = {
  assets: AssetRow[]
  used_by: Record<string, string[]>
  nodes: SyncNode[]
}

type IngestJob = {
  id: string
  filename: string
  state: string
  hash: string | null
  message: string | null
}

const shortHash = (hash: string) => hash.replace(/^[a-z0-9]+:/, '').slice(0, 8)

const size = (bytes: number) =>
  bytes >= 1024 * 1024
    ? `${(bytes / (1024 * 1024)).toFixed(1)} MB`
    : `${Math.max(1, Math.round(bytes / 1024))} KB`

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

/** A styled file input that reads as a row-level button. */
function UploadRow({
  label,
  accept,
  onFile,
}: {
  label: string
  accept: string
  onFile: (f: File) => void
}) {
  return (
    <label className="mdb__upload">
      <FileUp size={13} aria-hidden />
      <span>{label}</span>
      <input
        type="file"
        accept={accept}
        onChange={(e) => {
          const f = e.target.files?.[0]
          if (f) onFile(f)
          e.target.value = ''
        }}
      />
    </label>
  )
}

/**
 * A name a person can change, committed on blur or Enter.
 *
 * Uncontrolled and keyed by the stored name: the field belongs to whoever is
 * typing in it until they finish, and a refetch mid-edit must not reach in and
 * replace what they have half-written.
 */
function AssetName({
  hash,
  name,
  onRename,
}: {
  hash: string
  name: string
  onRename: (name: string) => void
}) {
  return (
    <input
      className="mdb__rename"
      key={`${hash}:${name}`}
      defaultValue={name}
      aria-label={`Name of ${shortHash(hash)}`}
      onKeyDown={(e) => {
        if (e.key === 'Enter') (e.target as HTMLInputElement).blur()
        if (e.key === 'Escape') {
          ;(e.target as HTMLInputElement).value = name
          ;(e.target as HTMLInputElement).blur()
        }
      }}
      onBlur={(e) => {
        const next = e.target.value.trim()
        if (!next || next === name) {
          e.target.value = name
          return
        }
        onRename(next)
      }}
    />
  )
}

type KindTab = 'all' | 'font' | 'image' | 'video'

export function MediaBrowser({
  faces,
  onClose,
}: {
  /** Parsed font identities, so a font row carries its file's own name. */
  faces: FaceInfo[]
  onClose: () => void
}) {
  const { data } = useQuery<SyncAnswer>(keys.assetSync())
  const [tab, setTab] = useState<KindTab>('all')
  const [problem, setProblem] = useState<string | null>(null)
  // Ingest jobs started FROM THIS DIALOG, polled until they settle. The
  // server keeps the full job list; this only follows what the user just did.
  const [jobs, setJobs] = useState<IngestJob[]>([])
  const pollTimer = useRef<ReturnType<typeof setInterval> | null>(null)

  // Possession changes while the dialog is open — that is what it is FOR.
  useEffect(() => {
    const t = setInterval(() => invalidate(keys.assetSync()), 5000)
    return () => clearInterval(t)
  }, [])

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => { if (e.key === 'Escape') onClose() }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [onClose])

  const rename = async (hash: string, name: string) => {
    setProblem(null)
    try {
      await patch(`/api/assets/${hash}`, { name })
      invalidate(keys.assets())
      invalidate(keys.assetSync())
    } catch (e) {
      setProblem(e instanceof Error ? e.message : String(e))
    }
  }

  const upload = async (file: File, kind: 'font' | 'image') => {
    setProblem(null)
    try {
      await postBytes(
        `/api/assets?kind=${kind}&name=${encodeURIComponent(file.name)}`,
        new Uint8Array(await file.arrayBuffer()),
      )
      invalidate(keys.assets())
      invalidate(keys.assetSync())
    } catch (e) {
      setProblem(`Could not add ${file.name}: ${e instanceof Error ? e.message : String(e)}`)
    }
  }

  const ingest = async (file: File) => {
    setProblem(null)
    try {
      const job = await postBytes<IngestJob>(
        `/api/assets/ingest?filename=${encodeURIComponent(file.name)}`,
        new Uint8Array(await file.arrayBuffer()),
      )
      setJobs((j) => [...j.filter((x) => x.id !== job.id), job])
    } catch (e) {
      setProblem(`Could not ingest ${file.name}: ${e instanceof Error ? e.message : String(e)}`)
    }
  }

  // Poll active jobs at 1.5 s until every one settles. An ingest of a long
  // clip is minutes; the row saying "converting" is what makes that fine.
  useEffect(() => {
    const active = jobs.some((j) => j.state !== 'done' && j.state !== 'failed')
    if (!active) {
      if (pollTimer.current) clearInterval(pollTimer.current)
      pollTimer.current = null
      return
    }
    if (pollTimer.current) return
    pollTimer.current = setInterval(async () => {
      const pending = jobs.filter((j) => j.state !== 'done' && j.state !== 'failed')
      for (const j of pending) {
        try {
          const fresh = (await (await fetch(keys.ingestJob(j.id))).json()) as IngestJob
          setJobs((all) => all.map((x) => (x.id === fresh.id ? fresh : x)))
          if (fresh.state === 'done') {
            invalidate(keys.assets())
            invalidate(keys.assetSync())
          }
        } catch {
          // The next tick retries; a poll miss is not a failed ingest.
        }
      }
    }, 1500)
    return () => {
      if (pollTimer.current) clearInterval(pollTimer.current)
      pollTimer.current = null
    }
  }, [jobs])

  const assets = data?.assets ?? []
  const nodes = data?.nodes ?? []
  const fonts = assets.filter((a) => a.kind === 'font')
  const images = assets.filter((a) => a.kind === 'image')
  const videos = assets.filter((a) => a.kind === 'video')
  const other = assets.filter((a) => !['font', 'image', 'video'].includes(a.kind))

  const row = (a: AssetRow, name: React.ReactNode, preview?: React.ReactNode, extra?: string) => (
    <div key={a.hash} className="mdb__row">
      {preview ?? <span className="mdb__swatch" aria-hidden />}
      <div className="mdb__name">
        <span>{name}</span>
        <span className="mdb__meta">
          {extra ? `${extra} · ` : ''}
          {shortHash(a.hash)} · {size(a.size)}
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

  const show = (k: KindTab) => tab === 'all' || tab === k

  return (
    <div className="mdb" role="dialog" aria-modal="true" aria-label="Media Browser" onClick={onClose}>
      <div className="mdb__panel mdb__panel--wide" onClick={(e) => e.stopPropagation()}>
        <header className="mdb__header">
          <span className="mdb__title">Media Browser</span>
          <div className="mdb__tabs" role="tablist" aria-label="Asset kind">
            {(['all', 'font', 'image', 'video'] as const).map((k) => (
              <button
                key={k}
                type="button"
                role="tab"
                aria-selected={tab === k}
                className={tab === k ? 'mdb__tab mdb__tab--on' : 'mdb__tab'}
                onClick={() => setTab(k)}
              >
                {k === 'all' ? 'All' : k === 'font' ? 'Fonts' : k === 'image' ? 'Images' : 'Videos'}
              </button>
            ))}
          </div>
          <button type="button" className="edit-console__iconbtn" aria-label="Close" onClick={onClose}>
            <X size={15} aria-hidden />
          </button>
        </header>

        <div className="mdb__body">
          {problem ? <p className="tl__problem" role="alert">{problem}</p> : null}
          {nodes.some((n) => n.staging && n.staging.pending > 0) ? (
            <p className="mdb__copying" role="status">
              {nodes
                .filter((n) => n.staging && n.staging.pending > 0)
                .map((n) =>
                  `Copying to ${n.name}: ${n.staging!.pending} file(s), ` +
                  `${Math.max(0, Math.round((n.staging!.total_bytes - n.staging!.done_bytes) / (1024 * 1024)))} MB left`)
                .join(' · ')}
            </p>
          ) : null}

          {show('font') ? (
            <>
              <GroupHeader label="Fonts" trailing={fonts.length ? `${fonts.length}` : undefined} />
              <UploadRow label="Add a font…" accept=".ttf,.otf,.ttc" onFile={(f) => void upload(f, 'font')} />
              {fonts.length === 0 ? <p className="mdb__dim">No fonts yet. A template renders only with a font this server holds.</p> : null}
              {fonts.map((a) => {
                const face = faces.find((f) => f.hash === a.hash)
                return row(a, face ? `${face.family} · ${face.weight}${face.italic ? ' italic' : ''}` : 'Unreadable font file')
              })}
            </>
          ) : null}

          {show('image') ? (
            <>
              <GroupHeader label="Images" trailing={images.length ? `${images.length}` : undefined} />
              <UploadRow label="Add an image…" accept=".png,.jpg,.jpeg" onFile={(f) => void upload(f, 'image')} />
              {images.length === 0 ? <p className="mdb__dim">No images yet.</p> : null}
              {images.map((a) =>
                row(
                  a,
                  <AssetName
                    hash={a.hash}
                    name={a.name ?? shortHash(a.hash)}
                    onRename={(n) => void rename(a.hash, n)}
                  />,
                  <img className="mdb__thumb" src={`/cas/${a.hash}`} alt="" />,
                ),
              )}
            </>
          ) : null}

          {show('video') ? (
            <>
              <GroupHeader label="Videos" trailing={videos.length ? `${videos.length}` : undefined} />
              <UploadRow label="Add a video…" accept="video/*,.mov,.mp4,.mxf" onFile={(f) => void ingest(f)} />
              <p className="mdb__dim">Long-GOP video is converted for frame-accurate playout.</p>
              {jobs
                .filter((j) => j.state !== 'done')
                .map((j) => (
                  <div key={j.id} className="mdb__row">
                    <span className="mdb__swatch" aria-hidden />
                    <div className="mdb__name">
                      <span>{j.filename}</span>
                      <span className="mdb__meta">
                        {j.state === 'failed' ? `Failed: ${j.message ?? 'no reason given'}` : `${j.state}…`}
                      </span>
                    </div>
                    <div className="mdb__chips">
                      <StatusPill tone={j.state === 'failed' ? 'muted' : 'staged'} size={10}>
                        {j.state.toUpperCase()}
                      </StatusPill>
                    </div>
                  </div>
                ))}
              {videos.length === 0 && jobs.length === 0 ? <p className="mdb__dim">No videos yet.</p> : null}
              {videos.map((a) =>
                row(
                  a,
                  <AssetName
                    hash={a.hash}
                    name={a.name ?? a.media?.filename ?? shortHash(a.hash)}
                    onRename={(n) => void rename(a.hash, n)}
                  />,
                  <span className="mdb__swatch mdb__swatch--clip" aria-hidden>
                    <Clapperboard size={13} />
                  </span>,
                  a.media
                    ? [
                        a.media.codec,
                        a.media.width && a.media.height ? `${a.media.width}×${a.media.height}` : null,
                        a.media.fps ? `${a.media.fps} fps` : null,
                        a.media.duration_s ? `${a.media.duration_s} s` : null,
                      ]
                        .filter(Boolean)
                        .join(' · ')
                    : undefined,
                ),
              )}
            </>
          ) : null}

          {tab === 'all' && other.length ? (
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
