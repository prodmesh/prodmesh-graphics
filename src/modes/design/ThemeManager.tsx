/**
 * Themes — create them, edit their tokens, and APPLY them (ADR 0021).
 *
 * Saving hashes the canonical bytes in Rust (the encoder behind
 * `template_rev`) and the server verifies the claim before storing. Applying
 * is the sweep: every template resolved under any version of this theme is
 * recompiled under the new tokens — client-side wasm, the same compile a
 * publish runs — republished, and its Items moved forward through
 * `update-items`, with per-template and per-Item verdicts. Nothing on air
 * changes until its next take; that is the compile-time promise.
 */

import { useEffect, useState } from 'react'
import { Button, FormGroup, GroupHeader } from '../../components/console'
import { get, post } from '../../lib/api'
import { invalidate } from '../../lib/useQuery'
import { keys } from '../../lib/keys'
import { compileTemplate, migrateDoc, rethemeDoc, themeHash } from './useDesign'
import { Plus, Trash2 } from 'lucide-react'

type ThemeToken = {
  color?: { rgba: [number, number, number, number] }
  font?: { family: string; weight: number }
}
type ThemeRow = {
  id: string
  name: string
  head_hash: string
  tokens: Record<string, ThemeToken>
  updated_at: number
}
type Follower = {
  id: string
  name: string
  head_rev: string
  theme_hash: string
  current: boolean
  mirror_hash: string | null
}

const hex = (c: [number, number, number, number]) =>
  `#${c
    .slice(0, 3)
    .map((v) => {
      // Stored linear-ish authored values are already encoded 0..1 here.
      const b = Math.round(Math.max(0, Math.min(1, v)) * 255)
      return b.toString(16).padStart(2, '0')
    })
    .join('')}`

const fromHex = (s: string): [number, number, number] => [
  parseInt(s.slice(1, 3), 16) / 255,
  parseInt(s.slice(3, 5), 16) / 255,
  parseInt(s.slice(5, 7), 16) / 255,
]

type SweepLine = { template: string; note: string; ok: boolean }

export function ThemeManager({
  themes,
  openDocId,
  onClose,
  onSwept,
}: {
  themes: ThemeRow[]
  /** The template open in the editor, so a sweep that touched it can say so. */
  openDocId: string | null
  onClose: () => void
  /** Templates the sweep republished — the owner refreshes anything open. */
  onSwept: (templateIds: string[]) => void
}) {
  const [selected, setSelected] = useState<string | null>(themes[0]?.id ?? null)
  const [name, setName] = useState('')
  const [tokens, setTokens] = useState<Record<string, ThemeToken>>({})
  const [newKey, setNewKey] = useState('')
  const [status, setStatus] = useState<string | null>(null)
  const [followers, setFollowers] = useState<Follower[] | null>(null)
  const [sweep, setSweep] = useState<SweepLine[] | null>(null)
  const [busy, setBusy] = useState(false)

  const row = themes.find((t) => t.id === selected) ?? null

  // Editing state follows the selection; a fresh selection is a fresh copy.
  useEffect(() => {
    setName(row?.name ?? '')
    setTokens(row ? structuredClone(row.tokens) : {})
    setFollowers(null)
    setSweep(null)
    setStatus(null)
  }, [selected, row])

  useEffect(() => {
    if (!row) return
    let live = true
    get<{ followers: Follower[] }>(`/api/themes/${row.id}/followers`)
      .then((r) => {
        if (live) setFollowers(r.followers)
      })
      .catch(() => {})
    return () => {
      live = false
    }
  }, [row])

  const save = async () => {
    setBusy(true)
    setStatus(null)
    try {
      const { hash, bytes } = await themeHash({ name: name.trim() || 'Untitled', tokens })
      const r = await post<{ id: string; head_hash: string }>('/api/themes', {
        id: selected ?? undefined,
        name: name.trim() || 'Untitled',
        tokens,
        hash,
        bytes: btoa(String.fromCharCode(...bytes)),
      })
      invalidate(keys.themes())
      setSelected(r.id)
      setStatus(`Saved. Theme revision ${hash.replace(/^[a-z0-9]+:/, '').slice(0, 8)}.`)
      const f = await get<{ followers: Follower[] }>(`/api/themes/${r.id}/followers`)
      setFollowers(f.followers)
    } catch (e) {
      setStatus(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  /**
   * The sweep. Per follower: mirror → migrate → re-resolve under the saved
   * theme → compile → publish → move Items. Per-template verdicts, and a
   * follower without an editable copy is REPORTED, not silently skipped.
   */
  const apply = async () => {
    if (!row || !followers) return
    setBusy(true)
    const lines: SweepLine[] = []
    const swept: string[] = []
    const themeJson = JSON.stringify({ name: row.name, tokens: row.tokens })
    for (const f of followers.filter((x) => !x.current)) {
      try {
        if (!f.mirror_hash) {
          lines.push({
            template: f.name,
            note: 'has no editable copy here. Republish it from the original file.',
            ok: false,
          })
          continue
        }
        const mirror = (await (await fetch(`/cas/${f.mirror_hash}`)).json()) as Record<string, unknown>
        const doc = await rethemeDoc(await migrateDoc(mirror), themeJson, row.head_hash)
        const compiled = await compileTemplate(doc)
        await post(`/api/templates/${f.id}/publish`, {
          scene: btoa(String.fromCharCode(...compiled.scene)),
          manifest: JSON.parse(compiled.manifest),
          mirror: JSON.stringify(doc),
        })
        const u = await post<{ updated: unknown[]; refused: { label: string; problem: string }[] }>(
          `/api/templates/${f.id}/update-items`,
          {},
        )
        const bits = [`republished as ${compiled.template_rev.replace(/^[a-z0-9]+:/, '').slice(0, 8)}`]
        if (u.updated.length) bits.push(`${u.updated.length} Item${u.updated.length === 1 ? '' : 's'} moved`)
        for (const ref of u.refused) bits.push(`${ref.label} refused: ${ref.problem}`)
        lines.push({ template: f.name, note: bits.join(' · '), ok: u.refused.length === 0 })
        swept.push(f.id)
      } catch (e) {
        lines.push({ template: f.name, note: e instanceof Error ? e.message : String(e), ok: false })
      }
    }
    setSweep(lines)
    setBusy(false)
    invalidate(keys.templates())
    invalidate(keys.items())
    const f = await get<{ followers: Follower[] }>(`/api/themes/${row.id}/followers`)
    setFollowers(f.followers)
    if (swept.length) onSwept(swept)
  }

  const stale = (followers ?? []).filter((f) => !f.current)

  return (
    <div className="mdb" role="dialog" aria-modal="true" aria-label="Themes" onClick={onClose}>
      <div className="mdb__panel" onClick={(e) => e.stopPropagation()}>
        <header className="mdb__header">
          <span className="mdb__title">Themes</span>
        </header>
        <div className="mdb__body">
          <div className="thm__pick">
            {themes.map((t) => (
              <button
                key={t.id}
                type="button"
                className={t.id === selected ? 'mdb__tab mdb__tab--on' : 'mdb__tab'}
                onClick={() => setSelected(t.id)}
              >
                {t.name}
              </button>
            ))}
            <button
              type="button"
              className="mdb__tab"
              onClick={() => {
                setSelected(null)
                setName('')
                setTokens({})
                setFollowers(null)
                setSweep(null)
              }}
            >
              New theme…
            </button>
          </div>

          <FormGroup label="Name">
            <input
              value={name}
              aria-label="Theme name"
              onChange={(e) => setName(e.target.value)}
            />
          </FormGroup>

          <GroupHeader label="Tokens" trailing={`${Object.keys(tokens).length}`} />
          {Object.entries(tokens).map(([key, tok]) => (
            <div key={key} className="thm__token">
              <span className="thm__key">{key}</span>
              {tok.color ? (
                <input
                  type="color"
                  value={hex(tok.color.rgba)}
                  aria-label={`Token ${key} colour`}
                  onChange={(e) => {
                    const [r, g, b] = fromHex(e.target.value)
                    setTokens((t) => ({
                      ...t,
                      [key]: { color: { rgba: [r, g, b, t[key]?.color?.rgba[3] ?? 1] } },
                    }))
                  }}
                />
              ) : tok.font ? (
                <>
                  <input
                    value={tok.font.family}
                    aria-label={`Token ${key} family`}
                    onChange={(e) =>
                      setTokens((t) => ({
                        ...t,
                        [key]: { font: { family: e.target.value, weight: t[key]?.font?.weight ?? 400 } },
                      }))
                    }
                  />
                  <input
                    type="number"
                    min={1}
                    max={1000}
                    step={100}
                    value={tok.font.weight}
                    aria-label={`Token ${key} weight`}
                    onChange={(e) =>
                      setTokens((t) => ({
                        ...t,
                        [key]: {
                          font: { family: t[key]?.font?.family ?? '', weight: Number(e.target.value) || 400 },
                        },
                      }))
                    }
                  />
                </>
              ) : null}
              <button
                type="button"
                className="edit-console__iconbtn"
                aria-label={`Remove token ${key}`}
                onClick={() =>
                  setTokens((t) => Object.fromEntries(Object.entries(t).filter(([k]) => k !== key)))
                }
              >
                <Trash2 size={12} aria-hidden />
              </button>
            </div>
          ))}
          <div className="thm__add">
            <input
              placeholder="token, e.g. brand.primary"
              aria-label="New token name"
              value={newKey}
              onChange={(e) => setNewKey(e.target.value)}
            />
            <button
              type="button"
              className="edit-console__addbtn"
              disabled={!newKey.trim() || newKey.trim() in tokens}
              onClick={() => {
                setTokens((t) => ({ ...t, [newKey.trim()]: { color: { rgba: [1, 1, 1, 1] } } }))
                setNewKey('')
              }}
            >
              <Plus size={12} aria-hidden /> Colour
            </button>
            <button
              type="button"
              className="edit-console__addbtn"
              disabled={!newKey.trim() || newKey.trim() in tokens}
              onClick={() => {
                setTokens((t) => ({ ...t, [newKey.trim()]: { font: { family: '', weight: 400 } } }))
                setNewKey('')
              }}
            >
              <Plus size={12} aria-hidden /> Font
            </button>
          </div>

          {row && followers ? (
            <>
              <GroupHeader label="Followed by" trailing={`${followers.length}`} />
              {followers.length === 0 ? (
                <p className="design-console__dim">No published template follows this theme yet.</p>
              ) : (
                followers.map((f) => (
                  <p key={f.id} className="thm__follower">
                    {f.name}
                    {f.current ? ' · current' : ' · older theme version'}
                    {openDocId === f.id ? ' · open in the editor' : ''}
                  </p>
                ))
              )}
              {stale.length ? (
                <p className="revs__stale">
                  {stale.length === 1
                    ? '1 template was resolved under an older version.'
                    : `${stale.length} templates were resolved under older versions.`}
                </p>
              ) : null}
            </>
          ) : null}

          {sweep ? (
            <div className="revs__result" role="status">
              {sweep.map((l) => (
                <p key={l.template} className={l.ok ? 'revs__ok' : 'revs__refused'}>
                  {l.template}: {l.note}
                </p>
              ))}
            </div>
          ) : null}
          {status ? <p className="design-console__dim" role="status">{status}</p> : null}
        </div>
        <footer className="revs__foot">
          <Button variant="secondary" disabled={busy} onClick={() => void save()}>
            {busy ? 'Working…' : selected ? 'Save theme' : 'Create theme'}
          </Button>
          {row && stale.length ? (
            <Button variant="secondary" disabled={busy} onClick={() => void apply()}>
              {busy
                ? 'Working…'
                : `Republish ${stale.length === 1 ? '1 template' : `${stale.length} templates`}`}
            </Button>
          ) : null}
          <Button variant="quiet" onClick={onClose}>Close</Button>
        </footer>
      </div>
    </div>
  )
}
