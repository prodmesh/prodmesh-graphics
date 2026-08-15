/**
 * Design — for the person who makes it look right.
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

import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { AlertTriangle, Check, Upload } from 'lucide-react'
import { useQuery, invalidate } from '../../lib/useQuery'
import { keys } from '../../lib/keys'
import { ApiError, get, post } from '../../lib/api'
import { compileTemplate, useDesignCanvas } from './useDesign'

type TemplateSummary = { id: string; name: string; head_rev: string | null }
type TemplateDetail = {
  head_rev: string | null
  head: { mirror_hash: string | null; manifest: { fields: { key: string; sample?: string }[] } } | null
}
type Asset = { hash: string; size: number; kind: string }

const FONT_KINDS = new Set(['font'])

export function Design() {
  const { data: templates } = useQuery<{ templates: TemplateSummary[] }>(keys.templates())
  const { data: assets } = useQuery<{ assets: Asset[] }>('/api/assets')

  const [selected, setSelected] = useState<string | null>(null)
  const [doc, setDoc] = useState<Record<string, unknown> | null>(null)
  const [data, setData] = useState<Record<string, string>>({})
  const [fonts, setFonts] = useState<Uint8Array[]>([])
  const [timeline, setTimeline] = useState<string | undefined>(undefined)
  const [tMs, setTMs] = useState(0)
  const [publishState, setPublishState] = useState<string | null>(null)

  const canvasRef = useRef<HTMLCanvasElement | null>(null)
  const { status, problem, info, paint } = useDesignCanvas(canvasRef, doc, data, fonts)

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
    setPublishState(null)
    if (!id) { setDoc(null); return }
    try {
      const t = await get<TemplateDetail>(keys.template(id))
      if (!t.head?.mirror_hash) {
        // The published revision is canonical CBOR, which this view cannot read
        // and deliberately never learns to — a JavaScript reader for the scene
        // format would be the second implementation of the thing that must
        // never disagree (ADR 0004). Without a mirror the file is the only way
        // back in, and saying so beats an empty canvas.
        setDoc(null)
        setPublishState(
          'This template was published without its editable copy. Open the original file to edit it.',
        )
        return
      }
      const mirror = await (await fetch(`/cas/${t.head.mirror_hash}`)).json()
      setDoc(mirror as Record<string, unknown>)
      setData(Object.fromEntries(
        (t.head.manifest.fields ?? []).map((f) => [f.key, f.sample ?? '']),
      ))
    } catch (e) {
      setPublishState(e instanceof ApiError ? e.message : String(e))
    }
  }, [])

  const timelines = useMemo(
    () => ((doc?.timelines as { name: string }[] | undefined) ?? []).map((t) => t.name),
    [doc],
  )

  const onOpenFile = useCallback(async (file: File) => {
    setPublishState(null)
    try {
      const json = JSON.parse(await file.text()) as Record<string, unknown>
      setDoc(json)
      setSelected(null)
      setData(Object.fromEntries(
        ((json.fields as { key: string; sample?: string }[] | undefined) ?? [])
          .map((f) => [f.key, f.sample ?? '']),
      ))
    } catch (e) {
      setPublishState(`That file is not a template: ${String(e)}`)
    }
  }, [])

  const onUploadFont = useCallback(async (file: File) => {
    const bytes = new Uint8Array(await file.arrayBuffer())
    const res = await fetch('/api/assets?kind=font', {
      method: 'POST',
      headers: { 'content-type': 'application/octet-stream' },
      body: bytes,
    })
    if (res.ok) invalidate('/api/assets')
  }, [])

  const publish = useCallback(async () => {
    if (!doc) return
    setPublishState('Compiling…')
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
      const bits = [`Published ${r.template_rev.slice(0, 14)}…`]
      if (r.warnings.length) bits.push(`${r.warnings.length} warning(s)`)
      // Named here, not discovered by a node at staging time on a Saturday.
      if (r.missing_assets.length) bits.push(`${r.missing_assets.length} asset(s) this server does not have`)
      setPublishState(bits.join(' · '))
    } catch (e) {
      setPublishState(e instanceof ApiError ? e.message : String(e))
    }
  }, [doc, list])

  return (
    <>
      <h1>Design</h1>

      <div className="design">
        <section className="design__stage" aria-label="Canvas">
          {/* The checkerboard is CSS. The canvas itself is transparent, because
              the output is a key and the designer must see what will key. */}
          <div className="design__checker">
            <canvas ref={canvasRef} className="design__canvas" />
          </div>

          {status === 'idle' ? (
            <p className="show__dim show__pad">
              Open a template file, and add a font, to see it render.
            </p>
          ) : null}
          {status === 'loading' ? <p className="show__dim show__pad">Rendering…</p> : null}
          {problem ? (
            <p className="show__problem" role="alert">
              <AlertTriangle size={15} aria-hidden /> {problem}
            </p>
          ) : null}

          {info?.degraded ? (
            <p className="show__problem" role="status">
              <AlertTriangle size={15} aria-hidden />
              Part of this template can’t be drawn by this version. It shows as magenta.
            </p>
          ) : null}
          {info?.omitted.length ? (
            // This is *why* the band got shorter. Guessing is much worse than
            // being told.
            <p className="show__dim show__pad">
              Hidden by a rule: {info.omitted.join(', ')}
            </p>
          ) : null}

          {timelines.length > 0 ? (
            <div className="design__scrub">
              <select
                value={timeline ?? ''}
                onChange={(e) => {
                  const v = e.target.value || undefined
                  setTimeline(v)
                  void paint(v, tMs)
                }}
                aria-label="Timeline"
              >
                <option value="">At rest</option>
                {timelines.map((t) => (
                  <option key={t} value={t}>{t}</option>
                ))}
              </select>
              <input
                type="range"
                min={0}
                max={2000}
                step={1}
                value={tMs}
                aria-label="Time"
                onChange={(e) => {
                  const v = Number(e.target.value)
                  setTMs(v)
                  void paint(timeline, v)
                }}
              />
              <span className="show__dim">{tMs} ms</span>
            </div>
          ) : null}
        </section>

        <aside className="design__panel">
          <label className="field">
            <span>Template file</span>
            <input
              type="file"
              accept=".json"
              onChange={(e) => { const f = e.target.files?.[0]; if (f) void onOpenFile(f) }}
            />
          </label>

          <label className="field">
            <span>Add a font</span>
            <input
              type="file"
              accept=".ttf,.otf,.ttc"
              onChange={(e) => { const f = e.target.files?.[0]; if (f) void onUploadFont(f) }}
            />
          </label>
          <p className="show__dim">
            {fontAssets.length === 0
              ? 'No fonts yet. A template can only render with a font this server holds — that is what makes it look the same on every machine.'
              : `${fontAssets.length} font(s) available`}
          </p>

          {list.length > 0 ? (
            <label className="field">
              <span>Published templates</span>
              <select value={selected ?? ''} onChange={(e) => void openTemplate(e.target.value)}>
                <option value="">—</option>
                {list.map((t) => (
                  <option key={t.id} value={t.id}>{t.name}</option>
                ))}
              </select>
            </label>
          ) : null}

          {doc ? (
            <>
              <h2 className="design__h2">Sample data</h2>
              {Object.keys(data).map((k) => (
                <label className="field" key={k}>
                  <span>{k}</span>
                  <input
                    value={data[k] ?? ''}
                    onChange={(e) => setData((d) => ({ ...d, [k]: e.target.value }))}
                  />
                </label>
              ))}

              <div className="edit__actions">
                <button type="button" className="btn btn--take" onClick={() => void publish()}>
                  <Check size={15} aria-hidden /> Publish
                </button>
              </div>
            </>
          ) : null}

          {publishState ? (
            <p className="design__status" role="status">
              <Upload size={14} aria-hidden /> {publishState}
            </p>
          ) : null}
        </aside>
      </div>
    </>
  )
}
