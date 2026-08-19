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
import { AlertTriangle, FileUp, FolderOpen, Image as ImageIcon, Plus, Redo2, Type, Undo2, Upload } from 'lucide-react'
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
  fontFacts,
  useDesignCanvas,
  type DesignSession,
  type DocTimeline,
  type FaceInfo,
  type Handles,
  type NodeFacts,
} from './useDesign'
import { MediaBrowser } from './MediaBrowser'
import { useDocHistory } from './useHistory'
import { LayerTree } from './LayerTree'
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
  faces,
  imageAssets,
  onEdit,
  onDelete,
}: {
  session: React.RefObject<DesignSession | null>
  ready: boolean
  /** The session's replacement counter — geometry is only new when it is. */
  generation: number
  selected: string
  /** The faces this server holds, by their files' own names — what the font
   *  picker offers. */
  faces: FaceInfo[]
  /** Image assets this server holds — what the picture picker offers. */
  imageAssets: { hash: string; size: number }[]
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
  const [expr, setExpr] = useState('')
  const [fontSize, setFontSize] = useState('')
  const [family, setFamily] = useState('')
  const [weight, setWeight] = useState('')

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
      setExpr(f.text?.expr ?? '')
      setFontSize(f.text ? String(f.text.size) : '')
      setFamily(f.text?.family ?? '')
      setWeight(f.text ? String(f.text.weight) : '')
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

  const commitExpr = () => {
    if (!facts?.text || expr === facts.text.expr) return
    edit((s) => s.setTextExpr(selected, expr))
  }

  const commitFontSize = () => {
    const v = Number(fontSize)
    if (!Number.isFinite(v) || v < 1 || v === facts?.text?.size) return
    edit((s) => s.setFontSize(selected, v))
  }

  const commitFont = (familyOverride?: string, weightOverride?: number) => {
    const fam = (familyOverride ?? family).trim()
    const wt = weightOverride ?? Number(weight)
    if (!facts?.text || !fam || !Number.isFinite(wt)) return
    if (fam === facts.text.family && wt === facts.text.weight) return
    // The nearest face in the picked family carries the hash — that is the
    // file a node will stage for this declaration.
    const match = faces
      .filter((f) => f.family === fam)
      .toSorted((a, b) => Math.abs(a.weight - wt) - Math.abs(b.weight - wt))[0]
    edit((s) =>
      s.setFontFamily(selected, fam, Math.max(1, Math.min(1000, Math.round(wt))), match?.hash),
    )
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

          {facts.image ? (
            <>
              <div className="design-console__size">
                <FormGroup label="Picture">
                  <select
                    value={facts.image.hash ?? ''}
                    aria-label="Picture asset"
                    onChange={(e) => { if (e.target.value) edit((s) => s.setImage(selected, e.target.value)) }}
                  >
                    {facts.image.hash && !imageAssets.some((a) => a.hash === facts.image!.hash) ? (
                      <option value={facts.image.hash}>
                        {shortRev(facts.image.hash)} — not on this server
                      </option>
                    ) : null}
                    {imageAssets.map((a) => (
                      <option key={a.hash} value={a.hash}>
                        {shortRev(a.hash)} · {Math.max(1, Math.round(a.size / 1024))} KB
                      </option>
                    ))}
                  </select>
                </FormGroup>
                <FormGroup label="Fit">
                  <select
                    value={facts.image.fit}
                    aria-label="Picture fit"
                    onChange={(e) => edit((s) => s.setFit(selected, e.target.value))}
                  >
                    <option value="cover">Scale to fill — crops</option>
                    <option value="contain">Scale to fit — letterboxes</option>
                    <option value="stretch">Stretch — distorts</option>
                  </select>
                </FormGroup>
              </div>
              {facts.kind === 'box' ? (
                <button
                  type="button"
                  className="edit-console__addbtn"
                  onClick={() => edit((s) => s.clearImage(selected))}
                >
                  Remove the image
                </button>
              ) : null}
            </>
          ) : facts.kind === 'box' && imageAssets.length > 0 ? (
            <button
              type="button"
              className="edit-console__addbtn"
              onClick={() => edit((s) => s.setImage(selected, imageAssets[0]!.hash))}
            >
              Add a background image
            </button>
          ) : null}

          {facts.text ? (
            <>
              {/* What it says. ${field} references bind Item data; typing one
                  declares the field, so publish never trips over bookkeeping
                  the designer was never shown. */}
              <FormGroup label="Text — ${field} binds Item data">
                <input
                  value={expr}
                  onChange={(e) => setExpr(e.target.value)}
                  onBlur={commitExpr}
                  onKeyDown={enterCommits(commitExpr)}
                  aria-label="Text content"
                />
              </FormGroup>
              <div className="design-console__size">
                <FormGroup label="Font family">
                  {/* A dropdown of what the server's font files call
                      THEMSELVES. Picking one carries the file's weight and
                      its hash into the declaration — the exact bytes a node
                      will stage. */}
                  <select
                    value={family}
                    aria-label="Font family"
                    onChange={(e) => {
                      const fam = e.target.value
                      // Of the picked family's faces, keep the weight closest
                      // to what the node already has — switching family should
                      // not also silently switch Regular to Bold.
                      const wt = Number(weight) || 400
                      const face = faces
                        .filter((f) => f.family === fam)
                        .toSorted((a, b) => Math.abs(a.weight - wt) - Math.abs(b.weight - wt))[0]
                      setFamily(fam)
                      if (face) setWeight(String(face.weight))
                      commitFont(fam, face?.weight)
                    }}
                  >
                    {family && !faces.some((f) => f.family === family) ? (
                      <option value={family}>{family} — no file on this server</option>
                    ) : null}
                    {[...new Set(faces.map((f) => f.family))].map((f) => (
                      <option key={f} value={f}>{f}</option>
                    ))}
                  </select>
                </FormGroup>
                <FormGroup label="Weight">
                  <input
                    type="number"
                    min={1}
                    max={1000}
                    step={100}
                    value={weight}
                    onChange={(e) => setWeight(e.target.value)}
                    onBlur={() => commitFont()}
                    onKeyDown={enterCommits(() => commitFont())}
                    aria-label="Font weight"
                  />
                </FormGroup>
              </div>
              <div className="design-console__size">
                <FormGroup label="Size">
                  <input
                    type="number"
                    min={1}
                    value={fontSize}
                    onChange={(e) => setFontSize(e.target.value)}
                    onBlur={commitFontSize}
                    onKeyDown={enterCommits(commitFontSize)}
                    aria-label="Font size"
                  />
                </FormGroup>
                <FormGroup label="Alignment">
                  <select
                    value={facts.text.align}
                    aria-label="Text alignment"
                    onChange={(e) => edit((s) => s.setTextAlign(selected, e.target.value))}
                  >
                    <option value="start">Left</option>
                    <option value="center">Centred</option>
                    <option value="end">Right</option>
                  </select>
                </FormGroup>
              </div>
            </>
          ) : null}

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
              {facts.text ? null : (
                <>
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
              )}
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
                {facts.text
                  ? 'Delete this text'
                  : facts.kind === 'image'
                    ? 'Delete this image'
                    : 'Delete this box — and everything inside it'}
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
  const [mediaOpen, setMediaOpen] = useState(false)

  // Image bytes by hash, for the assets THIS document references. The cache
  // outlives the state so an undo that re-references a picture never
  // refetches it; the state only changes when the referenced set does, so an
  // unrelated edit never re-arms over identical bytes.
  const imageCache = useRef<Record<string, Uint8Array>>({})
  const imageKeys = useRef('')
  const [images, setImages] = useState<Record<string, Uint8Array>>({})
  useEffect(() => {
    const wanted = [
      ...new Set(
        ((doc?.assets as { hash: string; kind: string }[] | undefined) ?? [])
          .filter((a) => a.kind === 'image')
          .map((a) => a.hash),
      ),
    ].sort()
    let cancelled = false
    ;(async () => {
      await Promise.all(
        wanted
          .filter((h) => !imageCache.current[h])
          .map(async (h) => {
            const r = await fetch(`/cas/${h}`)
            if (r.ok) imageCache.current[h] = new Uint8Array(await r.arrayBuffer())
          }),
      )
      if (cancelled) return
      const key = wanted.filter((h) => imageCache.current[h]).join(',')
      if (key === imageKeys.current) return
      imageKeys.current = key
      setImages(Object.fromEntries(wanted.filter((h) => imageCache.current[h]).map((h) => [h, imageCache.current[h]!])))
    })().catch(() => {})
    return () => { cancelled = true }
  }, [doc])

  const canvasRef = useRef<HTMLCanvasElement | null>(null)
  const { status, problem, info, paint, session, generation } = useDesignCanvas(canvasRef, doc, data, fonts, images)
  // The selected NODE on the canvas — a different thing from the selected
  // template in the browser rail, which predates it.
  const [selNode, setSelNode] = useState<string | null>(null)

  const list = templates?.templates ?? []
  const fontAssets = (assets?.assets ?? []).filter((a) => FONT_KINDS.has(a.kind))
  const imageAssets = (assets?.assets ?? []).filter((a) => a.kind === 'image')

  // Load the fonts this server holds, and read each file's OWN name — the
  // family a picker offers must be what the file states, not a typed string.
  // One fetch per hash, and they are immutable, so this never needs
  // invalidating.
  const [faces, setFaces] = useState<FaceInfo[]>([])
  useEffect(() => {
    if (fontAssets.length === 0) return
    let cancelled = false
    Promise.all(
      fontAssets.map(async (a) => {
        const bytes = new Uint8Array(await (await fetch(`/cas/${a.hash}`)).arrayBuffer())
        const info = await fontFacts(bytes)
        return { bytes, face: info ? { hash: a.hash, ...info } : null }
      }),
    ).then((loaded) => {
      if (cancelled) return
      setFonts(loaded.map((l) => l.bytes))
      setFaces(loaded.map((l) => l.face).filter((f): f is FaceInfo => f !== null))
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

  // ---- Playback -------------------------------------------------------------
  // Real time from the current playhead to the end, painted by rAF. This is a
  // preview of the motion, not the node's clock — the node schedules frames
  // against its output; this schedules them against the browser, which is
  // exactly right for judging an ease and nothing more.
  const [playing, setPlaying] = useState(false)
  const tMsRef = useRef(0)
  tMsRef.current = tMs

  useEffect(() => {
    if (!playing) return
    const tl = ((doc?.timelines as DocTimeline[] | undefined) ?? []).find(
      (t) => t.name === timeline,
    )
    const duration = tl?.duration_ms ?? 0
    if (!timeline || duration <= 0 || status !== 'ready') {
      setPlaying(false)
      return
    }
    // Space at the end means "again", not a one-frame no-op.
    const from = tMsRef.current >= duration - 1 ? 0 : tMsRef.current
    const t0 = performance.now()
    let raf = 0
    const step = () => {
      const t = Math.min(duration, from + (performance.now() - t0))
      setTMs(t)
      // paint() keeps only the LATEST queued request, so rAF outpacing the
      // GPU drops intermediate frames rather than piling them up.
      void paint(timeline, t)
      if (t >= duration) {
        setPlaying(false)
        return
      }
      raf = requestAnimationFrame(step)
    }
    raf = requestAnimationFrame(step)
    return () => cancelAnimationFrame(raf)
    // tMs deliberately absent: the loop OWNS the playhead while it runs, and
    // keying on it would restart the clock every frame.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [playing, timeline, status, doc, paint])

  /** Space: play the selected timeline, or the first one when none is picked —
   *  the habit every editor teaches, honoured before it is unlearned. */
  const togglePlay = useCallback(() => {
    if (timeline) {
      setPlaying((p) => !p)
      return
    }
    const names = ((doc?.timelines as DocTimeline[] | undefined) ?? []).map((t) => t.name)
    if (names.length === 0) return
    const pick = names.includes('In') ? 'In' : names[0]!
    setTimeline(pick)
    setTMs(0)
    setPlaying(true)
  }, [timeline, doc])

  useEffect(() => {
    // Not while a dialog is up: space playing an animation BEHIND the Media
    // Browser is motion nobody asked for.
    if (mediaOpen) return
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== ' ' || e.metaKey || e.ctrlKey || e.altKey) return
      const t = e.target as HTMLElement | null
      // A focused button keeps its native space-click, and a text control
      // keeps its space. Everything else: space is transport, never scroll.
      if (
        t &&
        (['INPUT', 'TEXTAREA', 'SELECT', 'BUTTON'].includes(t.tagName) || t.isContentEditable)
      ) {
        return
      }
      e.preventDefault()
      togglePlay()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [togglePlay, mediaOpen])

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

  const onUploadAsset = useCallback(async (file: File, kind: 'font' | 'image') => {
    const bytes = new Uint8Array(await file.arrayBuffer())
    const res = await fetch(`/api/assets?kind=${kind}`, {
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
  // The families this document declares, deduplicated — what the font picker
  // offers. Declaring a NEW one happens by typing it; the editor adds the
  // FontRef.
  /** Add a node — into the selection when there is one, else the canvas root —
   *  and select what was just made, so the next act is styling it. */
  const addNode = useCallback(
    (what: 'box' | 'text' | 'image') => {
      const s = session.current
      if (!s) return
      try {
        const raw =
          what === 'box'
            ? s.addBox(selNode ?? undefined)
            : what === 'text'
              ? s.addText(selNode ?? undefined)
              : s.addImage(selNode ?? undefined, imageAssets[0]!.hash)
        const r = JSON.parse(raw) as { id: string; doc: Record<string, unknown> }
        edit(r.doc)
        setSelNode(r.id)
      } catch (e) {
        // A failed edit is the designer's to know about, in words.
        setPublishState(`Could not add a ${what}: ${e instanceof Error ? e.message : String(e)}`)
      }
    },
    [session, selNode, imageAssets],
  )

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
          <Button variant="secondary" disabled={status !== 'ready'} onClick={() => addNode('box')}>
            <Plus size={14} aria-hidden /> Add a box
          </Button>
          <Button variant="secondary" disabled={status !== 'ready'} onClick={() => addNode('text')}>
            <Type size={14} aria-hidden /> Add text
          </Button>
          <Button
            variant="secondary"
            disabled={status !== 'ready' || imageAssets.length === 0}
            title={imageAssets.length === 0 ? 'Add an image asset first — right rail' : undefined}
            onClick={() => addNode('image')}
          >
            <ImageIcon size={14} aria-hidden /> Add an image
          </Button>
          <Button variant="secondary" onClick={() => setMediaOpen(true)}>
            <FolderOpen size={14} aria-hidden /> Media Browser
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
          {/* Structure below the browser, Figma-style: what is in THIS
              template, selectable and rearrangeable. Same selection as the
              canvas — one thing is selected, seen from two sides. */}
          <PanelHeader title="Layers" />
          <PanelBody>
            <LayerTree
              session={session}
              doc={doc}
              selected={selNode}
              onSelect={setSelNode}
              onEdit={edit}
            />
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
              playing={playing}
              onTogglePlay={togglePlay}
              onPickTimeline={(v) => {
                setPlaying(false)
                setTimeline(v)
                void paint(v, tMs)
              }}
              onScrub={(t) => {
                // Scrubbing takes the transport back: predictable beats clever.
                setPlaying(false)
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
                    faces={faces}
                    imageAssets={imageAssets}
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
                  onChange={(e) => { const f = e.target.files?.[0]; if (f) void onUploadAsset(f, 'font') }}
                />
              </label>
              <p className="design-console__dim">
                {fontAssets.length === 0
                  ? 'No fonts yet. A template can only render with a font this server holds — that is what makes it look the same on every machine.'
                  : 'Fonts are carried by content hash and staged to nodes like any other asset.'}
              </p>

              <GroupHeader
                label="Images"
                trailing={imageAssets.length ? `${imageAssets.length}` : undefined}
              />
              <label className="design-console__file">
                <FileUp size={14} aria-hidden />
                <span>Add an image…</span>
                <input
                  type="file"
                  accept=".png,.jpg,.jpeg"
                  onChange={(e) => { const f = e.target.files?.[0]; if (f) void onUploadAsset(f, 'image') }}
                />
              </label>
              <p className="design-console__dim">
                {imageAssets.length === 0
                  ? 'No images yet. Add one to place it on the canvas or behind a box.'
                  : 'Pictures are carried by content hash, so a template shows the same pixels everywhere.'}
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

      {mediaOpen ? <MediaBrowser faces={faces} onClose={() => setMediaOpen(false)} /> : null}
    </div>
  )
}
