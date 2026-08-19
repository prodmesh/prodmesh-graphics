/**
 * Design — for the person who makes it look right.
 *
 * The console layout (ADR 0020): template browser in the left rail, the canvas
 * in the centre workspace, template facts and publishing in the right
 * inspector. Show has no route here, on purpose (ADR 0015).
 *
 * # The canvas is the output, not a picture of it
 *
 * It renders with `pmg-render` compiled to WASM — the same code the playout
 * node runs. That is the whole premise of ADR 0001, and it is what makes the
 * preview trustworthy rather than indicative. The `template_rev` this mode
 * computes when it publishes is byte-identical to the one `pmgctl` computes for
 * the same document.
 *
 * # It is drawn on a checkerboard, in CSS
 *
 * The output is a **key signal**, so the canvas is cleared to transparent and
 * the checkerboard lives behind it in a stylesheet. Baking a background into
 * the pixels would hide exactly the thing a designer needs to see — what will
 * actually key over live video.
 *
 * # Fonts come from the CAS
 *
 * There is no system font database. A template naming a system font would
 * render one way here and another on a booth Mac, silently. Fonts are assets
 * carried by content hash, and this mode fetches them the same way a node does.
 *
 * Every operation is reachable without a drag: jsdom has no layout, so a
 * simulated drag certifies nothing, and the button-and-keyboard paths are what
 * the tests can actually cover.
 */

import { useCallback, useEffect, useRef, useState } from 'react'
import { AlertTriangle, FileUp, Plus, Redo2, Undo2, Upload } from 'lucide-react'
import { useQuery, invalidate } from '../../lib/useQuery'
import { keys } from '../../lib/keys'
import { ApiError, get, post } from '../../lib/api'
import {
  Button,
  FormGroup,
  GroupHeader,
  KeyValueRow,
  NavItem,
  Panel,
  PanelBody,
  PanelHeader,
  StatusPill,
  Tag,
} from '../../components/console'
import {
  compileTemplate,
  useDesignCanvas,
  type DesignSession,
  type Handles,
  type NodeFacts,
} from './useDesign'
import { useDocHistory } from './useHistory'
import { TimelinePanel } from './TimelinePanel'
import { CanvasStage } from './CanvasStage'
import type { Item } from '../show/types'

type TemplateSummary = { id: string; name: string; head_rev: string | null }
type TemplateDetail = {
  head_rev: string | null
  head: { mirror_hash: string | null; manifest: { fields: { key: string; sample?: string }[] } } | null
}
type Asset = { hash: string; size: number; kind: string }

const FONT_KINDS = new Set(['font'])

/** Encoded 0..1 channel to a two-digit hex pair. */
const hex2 = (v: number) =>
  Math.round(Math.max(0, Math.min(1, v)) * 255)
    .toString(16)
    .padStart(2, '0')

const rgbToHex = (c: [number, number, number, number]) => `#${hex2(c[0])}${hex2(c[1])}${hex2(c[2])}`

const hexToRgb = (hex: string): [number, number, number] | null => {
  const m = /^#([0-9a-f]{6})$/i.exec(hex)
  if (!m) return null
  const n = parseInt(m[1]!, 16)
  return [((n >> 16) & 255) / 255, ((n >> 8) & 255) / 255, (n & 255) / 255]
}

/**
 * The selected node: solved geometry above, authored properties below.
 *
 * X/Y are read from the solved geometry — where the node actually is, which
 * with flexbox is an answer, not a setting. Everything editable writes back
 * through the same session the handles use, so the inspector and the canvas
 * cannot disagree about what an edit means. The colour input speaks the
 * document's encoded values directly: what the designer picks is what an Item
 * author later reads.
 */
function SelectedNode({
  session,
  ready,
  generation,
  selected,
  onEdit,
  onDelete,
}: {
  session: React.RefObject<DesignSession | null>
  ready: boolean
  /** The session's replacement counter — geometry is only new when it is. */
  generation: number
  selected: string
  onEdit: (doc: Record<string, unknown>) => void
  /** Absent for the root: the root is the canvas, not a thing to delete. */
  onDelete?: () => void
}) {
  const [bbox, setBbox] = useState<[number, number, number, number] | null>(null)
  const [facts, setFacts] = useState<NodeFacts | null>(null)
  const [w, setW] = useState('')
  const [h, setH] = useState('')
  const [radius, setRadius] = useState('')
  const [opacity, setOpacity] = useState('')
  const [alpha, setAlpha] = useState('')

  useEffect(() => {
    const s = session.current
    if (!ready || !s) return
    try {
      const hd = JSON.parse(s.handles(selected)) as Handles
      setBbox(hd.bbox)
      setW(String(Math.round(hd.bbox[2])))
      setH(String(Math.round(hd.bbox[3])))
      const f = JSON.parse(s.facts(selected)) as NodeFacts
      setFacts(f)
      setRadius(String(f.radius))
      setOpacity(String(Math.round(f.opacity * 100)))
      setAlpha(String(Math.round((f.fill?.[3] ?? 1) * 100)))
    } catch {
      setBbox(null)
      setFacts(null)
    }
  }, [session, ready, generation, selected])

  /** Run one session edit and hand the edited document up. */
  const edit = (fn: (s: DesignSession) => string) => {
    const s = session.current
    if (!s) return
    try {
      onEdit(JSON.parse(fn(s)) as Record<string, unknown>)
    } catch {
      // The node may be gone; the selection clears upstream.
    }
  }

  const commitSize = () => {
    const wv = Number(w)
    const hv = Number(h)
    if (!Number.isFinite(wv) || !Number.isFinite(hv) || wv < 1 || hv < 1) return
    if (bbox && Math.round(bbox[2]) === wv && Math.round(bbox[3]) === hv) return
    edit((s) => s.setSize(selected, wv, hv))
  }

  const commitRadius = () => {
    const r = Number(radius)
    if (!Number.isFinite(r) || r < 0 || r === facts?.radius) return
    edit((s) => s.setRadius(selected, r))
  }

  const commitOpacity = () => {
    const o = Number(opacity)
    if (!Number.isFinite(o)) return
    const v = Math.max(0, Math.min(100, o)) / 100
    if (facts && Math.round(facts.opacity * 100) === Math.round(v * 100)) return
    edit((s) => s.setOpacity(selected, v))
  }

  const commitFill = (hexOverride?: string, alphaOverride?: string) => {
    if (!facts?.fill) return
    const rgb = hexToRgb(hexOverride ?? rgbToHex(facts.fill))
    const a = Number(alphaOverride ?? alpha)
    if (!rgb || !Number.isFinite(a)) return
    edit((s) => s.setFill(selected, rgb[0], rgb[1], rgb[2], Math.max(0, Math.min(100, a)) / 100))
  }

  const enterCommits = (commit: () => void) => (e: React.KeyboardEvent) => {
    if (e.key === 'Enter') commit()
  }

  return (
    <div className="design-console__selected">
      <GroupHeader label="Selected" trailing={selected} />
      {bbox && facts ? (
        <>
          <KeyValueRow label="Position" value={`${Math.round(bbox[0])}, ${Math.round(bbox[1])}`} tone="plain" />

          {/* The pin. Its own explicit control, because it changes what a
              drag DOES: pinned moves freely, in-flow moves shift neighbours. */}
          <FormGroup label="Placement">
            <select
              value={facts.absolute ? 'absolute' : 'relative'}
              aria-label="Placement"
              onChange={(e) => edit((s) => s.setPosition(selected, e.target.value === 'absolute'))}
            >
              <option value="relative">In the flow — neighbours move with it</option>
              <option value="absolute">Pinned — moves freely</option>
            </select>
          </FormGroup>

          <div className="design-console__size">
            <FormGroup label="Width">
              <input
                type="number"
                min={1}
                value={w}
                onChange={(e) => setW(e.target.value)}
                onBlur={commitSize}
                onKeyDown={enterCommits(commitSize)}
                aria-label="Selected width"
              />
            </FormGroup>
            <FormGroup label="Height">
              <input
                type="number"
                min={1}
                value={h}
                onChange={(e) => setH(e.target.value)}
                onBlur={commitSize}
                onKeyDown={enterCommits(commitSize)}
                aria-label="Selected height"
              />
            </FormGroup>
          </div>

          {facts.fill ? (
            <>
              <div className="design-console__size">
                <FormGroup label="Fill">
                  <input
                    type="color"
                    value={rgbToHex(facts.fill)}
                    onChange={(e) => commitFill(e.target.value)}
                    aria-label="Fill colour"
                  />
                </FormGroup>
                <FormGroup label="Fill opacity %">
                  <input
                    type="number"
                    min={0}
                    max={100}
                    value={alpha}
                    onChange={(e) => setAlpha(e.target.value)}
                    onBlur={() => commitFill()}
                    onKeyDown={enterCommits(() => commitFill())}
                    aria-label="Fill opacity"
                  />
                </FormGroup>
              </div>
              <button
                type="button"
                className="edit-console__addbtn"
                onClick={() => edit((s) => s.clearFill(selected))}
              >
                Remove the fill
              </button>
              <p className="design-console__dim">
                A box with no fill draws nothing and stops being clickable on the canvas —
                select it again from here if you change your mind.
              </p>
            </>
          ) : (
            <button
              type="button"
              className="edit-console__addbtn"
              onClick={() => edit((s) => s.setFill(selected, 1, 1, 1, 1))}
            >
              Add a fill
            </button>
          )}

          <div className="design-console__size">
            <FormGroup label="Corner radius">
              <input
                type="number"
                min={0}
                value={radius}
                onChange={(e) => setRadius(e.target.value)}
                onBlur={commitRadius}
                onKeyDown={enterCommits(commitRadius)}
                aria-label="Corner radius"
              />
            </FormGroup>
            <FormGroup label="Opacity %">
              <input
                type="number"
                min={0}
                max={100}
                value={opacity}
                onChange={(e) => setOpacity(e.target.value)}
                onBlur={commitOpacity}
                onKeyDown={enterCommits(commitOpacity)}
                aria-label="Node opacity"
              />
            </FormGroup>
          </div>

          <p className="design-console__dim">
            Moving edits margins: a pinned node moves freely; one in the flow adjusts its
            place, so its neighbours move with it — that is the layout being responsive,
            not the editor being wrong.
          </p>

          {onDelete ? (
            <>
              <Button variant="secondary" block onClick={onDelete}>
                Delete this box — and everything inside it
              </Button>
              <p className="design-console__dim">Also on the keyboard: Delete, with the canvas focused.</p>
            </>
          ) : null}
        </>
      ) : null}
    </div>
  )
}

/**
 * A revision, shortened for the eye. The full value is an address whose
 * algorithm prefix is an internal fact (docs/UI_TEXT.md bans it on screen);
 * eight characters is plenty to tell two revisions apart in a list.
 */
const shortRev = (rev: string) => rev.replace(/^[a-z0-9]+:/, '').slice(0, 8)

export function Design() {
  const { data: templates } = useQuery<{ templates: TemplateSummary[] }>(keys.templates())
  const { data: assets } = useQuery<{ assets: Asset[] }>(keys.assets())
  const { data: items } = useQuery<{ items: Item[] }>(keys.items())

  const [selected, setSelected] = useState<string | null>(null)
  // The document, behind its undo history: `edit` for changes, `load` for
  // opening — a load starts a new timeline (ADR 0019: undo operates on the
  // SceneDoc, and lives where the document does).
  const { doc, load, edit, undo, redo, canUndo, canRedo } = useDocHistory()
  const [data, setData] = useState<Record<string, string>>({})
  const [fonts, setFonts] = useState<Uint8Array[]>([])
  const [timeline, setTimeline] = useState<string | undefined>(undefined)
  const [tMs, setTMs] = useState(0)
  const [publishState, setPublishState] = useState<string | null>(null)

  const canvasRef = useRef<HTMLCanvasElement | null>(null)
  const { status, problem, info, paint, session, generation } = useDesignCanvas(canvasRef, doc, data, fonts)
  // The selected NODE on the canvas — a different thing from the selected
  // template in the browser rail, which predates it.
  const [selNode, setSelNode] = useState<string | null>(null)

  const list = templates?.templates ?? []
  const fontAssets = (assets?.assets ?? []).filter((a) => FONT_KINDS.has(a.kind))

  // Load the fonts this server holds. One fetch per hash, and they are
  // immutable, so this never needs invalidating.
  useEffect(() => {
    if (fontAssets.length === 0) return
    let cancelled = false
    Promise.all(
      fontAssets.map((a) => fetch(`/cas/${a.hash}`).then((r) => r.arrayBuffer())),
    ).then((bufs) => {
      if (!cancelled) setFonts(bufs.map((b) => new Uint8Array(b)))
    }).catch(() => {})
    return () => { cancelled = true }
  }, [assets])

  // Load the selected template's published document, via its revision — which
  // IS its address in the CAS.
  const openTemplate = useCallback(async (id: string) => {
    setSelected(id)
    setSelNode(null)
    setPublishState(null)
    if (!id) { load(null); return }
    try {
      const t = await get<TemplateDetail>(keys.template(id))
      if (!t.head?.mirror_hash) {
        // The published revision is canonical CBOR, which this view cannot read
        // and deliberately never learns to — a JavaScript reader for the scene
        // format would be the second implementation of the thing that must
        // never disagree (ADR 0004). Without a mirror the file is the only way
        // back in, and saying so beats an empty canvas.
        load(null)
        setPublishState(
          'This template was published without its editable copy. Open the original file to edit it.',
        )
        return
      }
      const mirror = await (await fetch(`/cas/${t.head.mirror_hash}`)).json()
      load(mirror as Record<string, unknown>)
      setData(Object.fromEntries(
        (t.head.manifest.fields ?? []).map((f) => [f.key, f.sample ?? '']),
      ))
    } catch (e) {
      setPublishState(e instanceof ApiError ? e.message : String(e))
    }
  }, [load])

  // After every re-arm, repaint the moment the designer is LOOKING AT — a key
  // edit while scrubbed to 300 ms must update that frame, not snap the canvas
  // back to rest. (The hook paints rest after arming; paint() queues the
  // latest request behind an in-flight one, so this wins.)
  useEffect(() => {
    if (status === 'ready' && (timeline !== undefined || tMs !== 0)) void paint(timeline, tMs)
    // Deliberately keyed on re-arms alone.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [generation])

  // Cmd/Ctrl+Z and Shift+Cmd/Ctrl+Z (plus Ctrl+Y), anywhere in Design except
  // inside a text control — a field keeps its own native undo, because taking
  // it over would make typing corrections rewind the document.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!(e.metaKey || e.ctrlKey)) return
      const key = e.key.toLowerCase()
      if (key !== 'z' && key !== 'y') return
      const t = e.target as HTMLElement | null
      if (t && ['INPUT', 'TEXTAREA', 'SELECT'].includes(t.tagName)) return
      e.preventDefault()
      if (key === 'y' || e.shiftKey) redo()
      else undo()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [undo, redo])

  const onOpenFile = useCallback(async (file: File) => {
    setPublishState(null)
    try {
      const json = JSON.parse(await file.text()) as Record<string, unknown>
      load(json)
      setSelected(null)
      setSelNode(null)
      setData(Object.fromEntries(
        ((json.fields as { key: string; sample?: string }[] | undefined) ?? [])
          .map((f) => [f.key, f.sample ?? '']),
      ))
    } catch (e) {
      setPublishState(`That file is not a template: ${String(e)}`)
    }
  }, [load])

  const onUploadFont = useCallback(async (file: File) => {
    const bytes = new Uint8Array(await file.arrayBuffer())
    const res = await fetch('/api/assets?kind=font', {
      method: 'POST',
      headers: { 'content-type': 'application/octet-stream' },
      body: bytes,
    })
    if (res.ok) invalidate(keys.assets())
  }, [])

  const publish = useCallback(async () => {
    if (!doc) return
    setPublishState('Publishing…')
    try {
      // Compiled HERE, by the same Rust the CLI runs. The server stores the
      // bytes opaquely and checks only that their address matches the manifest.
      const compiled = await compileTemplate(doc)
      const id = String(doc.id ?? '')
      if (!list.some((t) => t.id === id)) {
        await post(keys.templates(), { id, name: String(doc.name ?? 'Untitled') })
      }
      const r = await post<{ template_rev: string; warnings: { code: string; detail: string }[]; missing_assets: string[] }>(
        `/api/templates/${id}/publish`,
        {
          scene: btoa(String.fromCharCode(...compiled.scene)),
          manifest: JSON.parse(compiled.manifest),
          // The editable copy travels with the publish, so this template can be
          // reopened on a machine that has never seen the original file.
          mirror: JSON.stringify(doc),
        },
      )
      invalidate(keys.templates())
      const bits = [`Published — revision ${shortRev(r.template_rev)}`]
      if (r.warnings.length) bits.push(`${r.warnings.length} warning(s)`)
      // Named here, not discovered by a node at staging time on a Saturday.
      if (r.missing_assets.length) bits.push(`${r.missing_assets.length} asset(s) this server does not have`)
      setPublishState(bits.join(' · '))
    } catch (e) {
      setPublishState(e instanceof ApiError ? e.message : String(e))
    }
  }, [doc, list])

  const docId = doc ? String(doc.id ?? '') : null
  const usedBy = docId ? (items?.items ?? []).filter((i) => i.template_id === docId).length : 0
  const published = docId ? list.some((t) => t.id === docId && t.head_rev) : false
  const rootId = doc ? ((doc.root as { id?: string } | undefined)?.id ?? null) : null

  /** Add a box — into the selection when there is one, else the canvas root —
   *  and select what was just made, so the next act is styling it. */
  const addBox = useCallback(() => {
    const s = session.current
    if (!s) return
    try {
      const r = JSON.parse(s.addBox(selNode ?? undefined)) as {
        id: string
        doc: Record<string, unknown>
      }
      edit(r.doc)
      setSelNode(r.id)
    } catch (e) {
      // A failed edit is the designer's to know about, in words.
      setPublishState(`Could not add a box: ${e instanceof Error ? e.message : String(e)}`)
    }
  }, [session, selNode])

  /** Delete the selection and everything inside it. The root is the canvas
   *  and is never offered for deletion. */
  const deleteSelected = useCallback(() => {
    const s = session.current
    if (!s || !selNode || selNode === rootId) return
    try {
      edit(JSON.parse(s.deleteNode(selNode)) as Record<string, unknown>)
      setSelNode(null)
    } catch {
      // The node may already be gone; the selection effect clears it.
    }
  }, [session, selNode, rootId])

  return (
    <div className="design-console">
      <div className="console__toolbar">
        <div className="console__toolbar-group">
          <Button disabled={!doc} onClick={() => void publish()}>
            <Upload size={14} aria-hidden /> Publish
          </Button>
          <Button variant="secondary" disabled={status !== 'ready'} onClick={addBox}>
            <Plus size={14} aria-hidden /> Add a box
          </Button>
          <Button variant="secondary" disabled={!canUndo} onClick={undo} title="Undo (Cmd+Z)">
            <Undo2 size={14} aria-hidden /> Undo
          </Button>
          <Button variant="secondary" disabled={!canRedo} onClick={redo} title="Redo (Shift+Cmd+Z)">
            <Redo2 size={14} aria-hidden /> Redo
          </Button>
          <span className="console__toolbar-note">
            Publishing is what makes a Template usable by Items. Nodes stage it by its address.
          </span>
        </div>
        <Tag tone="raised" size={10}>{`${list.length} templates`}</Tag>
      </div>

      <div className="design-console__panels">
        <Panel width="var(--pm-rail-width-edit)" label="Template browser">
          <PanelHeader title="Template Browser" />
          <PanelBody>
            <label className="design-console__file">
              <FileUp size={14} aria-hidden />
              <span>Open a template file…</span>
              <input
                type="file"
                accept=".json"
                onChange={(e) => { const f = e.target.files?.[0]; if (f) void onOpenFile(f) }}
              />
            </label>
            {list.length === 0 ? (
              <p className="design-console__dim">
                Nothing published yet. Open a template file and publish it.
              </p>
            ) : (
              <>
                <GroupHeader label="Published" trailing={`${list.length}`} />
                {list.map((t) => (
                  <NavItem
                    key={t.id}
                    title={t.name}
                    meta={t.head_rev ? `revision ${shortRev(t.head_rev)}` : 'no revision'}
                    active={selected === t.id}
                    onClick={() => void openTemplate(t.id)}
                  />
                ))}
              </>
            )}
          </PanelBody>
        </Panel>

        <div className="console__center design-console__center">
          <div className="design-console__stage" aria-label="Canvas">
            {/* The checkerboard is CSS. The canvas itself is transparent, because
                the output is a key and the designer must see what will key.
                Geometry questions — what is under the pointer, where handles
                go, what a drag writes — are answered by Rust (ADR 0019); the
                stage draws the chrome at the coordinates it reports. */}
            <CanvasStage
              canvasRef={canvasRef}
              session={session}
              ready={status === 'ready'}
              generation={generation}
              selected={selNode}
              onSelect={setSelNode}
              onEdit={edit}
              onDelete={deleteSelected}
            />

            {status === 'idle' ? (
              <p className="design-console__dim">
                Open a template file, and add a font, to see it render.
              </p>
            ) : null}
            {status === 'loading' ? <p className="design-console__dim">Rendering…</p> : null}
            {problem ? (
              <p className="design-console__problem" role="alert">
                <AlertTriangle size={15} aria-hidden /> {problem}
              </p>
            ) : null}

            {info?.degraded ? (
              <p className="design-console__problem" role="status">
                <AlertTriangle size={15} aria-hidden />
                Part of this template can’t be drawn by this version. It shows as magenta.
              </p>
            ) : null}
            {info?.omitted.length ? (
              // This is *why* the band got shorter. Guessing is much worse than
              // being told.
              <p className="design-console__dim">Hidden by a rule: {info.omitted.join(', ')}</p>
            ) : null}
          </div>

          <div className="design-console__timeline">
            <TimelinePanel
              session={session}
              ready={status === 'ready'}
              doc={doc}
              selected={selNode}
              timeline={timeline}
              tMs={tMs}
              onPickTimeline={(v) => {
                setTimeline(v)
                void paint(v, tMs)
              }}
              onScrub={(t) => {
                setTMs(t)
                void paint(timeline, t)
              }}
              onEdit={edit}
            />
          </div>
        </div>

        <Panel width="var(--pm-inspector-width)" label="Template and publish">
          <PanelHeader
            title="Template & Publish"
            trailing={
              doc ? (
                <StatusPill tone={published ? 'ready' : 'staged'} variant="subtle" size={10}>
                  {published ? 'PUBLISHED' : 'NOT PUBLISHED'}
                </StatusPill>
              ) : undefined
            }
          />
          <PanelBody pad16>
            {doc ? (
              <>
                <div className="design-console__name">
                  <span className="design-console__title">{String(doc.name ?? 'Untitled')}</span>
                  <span className="design-console__id">{docId}</span>
                </div>

                <KeyValueRow
                  label="Used by"
                  value={usedBy === 1 ? '1 Item' : `${usedBy} Items`}
                  tone={usedBy ? 'armed' : 'plain'}
                />

                {selNode ? (
                  <SelectedNode
                    session={session}
                    ready={status === 'ready'}
                    generation={generation}
                    selected={selNode}
                    onEdit={edit}
                    onDelete={selNode === rootId ? undefined : deleteSelected}
                  />
                ) : (
                  <p className="design-console__dim">
                    Click a box on the canvas to select it. Drag to move, drag a handle to
                    resize, arrows nudge. Hold Alt to ignore snapping.
                  </p>
                )}

                <GroupHeader label="Sample data" />
                {Object.keys(data).map((k) => (
                  <FormGroup key={k} label={k}>
                    <input
                      value={data[k] ?? ''}
                      onChange={(e) => setData((d) => ({ ...d, [k]: e.target.value }))}
                    />
                  </FormGroup>
                ))}
              </>
            ) : (
              <p className="design-console__dim">
                Open a template from the browser or from a file. There is no canvas interaction
                yet — no selection, no moving a box, no property editing.
              </p>
            )}

            <div className="design-console__fonts">
              <GroupHeader
                label="Fonts"
                trailing={fontAssets.length ? `${fontAssets.length}` : undefined}
              />
              <label className="design-console__file">
                <FileUp size={14} aria-hidden />
                <span>Add a font…</span>
                <input
                  type="file"
                  accept=".ttf,.otf,.ttc"
                  onChange={(e) => { const f = e.target.files?.[0]; if (f) void onUploadFont(f) }}
                />
              </label>
              <p className="design-console__dim">
                {fontAssets.length === 0
                  ? 'No fonts yet. A template can only render with a font this server holds — that is what makes it look the same on every machine.'
                  : 'Fonts are carried by content hash and staged to nodes like any other asset.'}
              </p>
            </div>

            {publishState ? (
              <p className="design-console__status" role="status">
                <Upload size={14} aria-hidden /> {publishState}
              </p>
            ) : null}
          </PanelBody>
        </Panel>
      </div>
    </div>
  )
}
