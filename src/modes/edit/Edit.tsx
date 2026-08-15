/**
 * Edit — for the person preparing this week's service.
 *
 * Items: data bound to a Template plus a destination. Playlists and the Rundown
 * are M2.
 *
 * # Two things this mode is responsible for saying out loud
 *
 * **Readiness is not advisory.** An Item whose node has not finished staging
 * its assets is *not armable*, and Thursday evening is when that must be
 * visible — not Sunday at 10:15.
 *
 * **An Item is pinned to a template revision.** Republishing a template does
 * not change existing Items, which is what stops a design tweak from silently
 * altering a rundown. When an Item is behind head, this mode says so and moving
 * it forward is a deliberate click.
 */

import { useCallback, useEffect, useState } from 'react'
import { AlertTriangle, Check, Plus } from 'lucide-react'
import { useQuery, invalidate } from '../../lib/useQuery'
import { useTopic } from '../../lib/stream'
import { keys, topics } from '../../lib/keys'
import { ApiError, patch, post } from '../../lib/api'
import type { Item } from '../show/types'

type Field = {
  key: string
  type: string
  label?: string
  required?: boolean
  max_len?: number
  sample?: string
  options?: string[]
}

type TemplateSummary = { id: string; name: string; head_rev: string | null; revisions: number }
type TemplateDetail = { id: string; name: string; head_rev: string | null; head: { manifest: { fields: Field[] } } | null }

export function Edit() {
  const { data: templates } = useQuery<{ templates: TemplateSummary[] }>(keys.templates())
  const { data: items } = useQuery<{ items: Item[] }>(keys.items())
  const [editing, setEditing] = useState<string | 'new' | null>(null)

  const publishable = (templates?.templates ?? []).filter((t) => t.head_rev)

  return (
    <>
      <h1>Edit</h1>

      {publishable.length === 0 ? (
        <div className="mode__stub">
          <p>No template has been published yet.</p>
          <p>Build one in Design and publish it, then items can be built from it here.</p>
        </div>
      ) : (
        <div className="edit">
          <div className="edit__list">
            <div className="edit__list-head">
              <h2>Items</h2>
              <button type="button" className="btn btn--ghost" onClick={() => setEditing('new')}>
                <Plus size={15} aria-hidden /> New item
              </button>
            </div>

            {(items?.items ?? []).length === 0 ? (
              <p className="show__dim show__pad">Nothing yet.</p>
            ) : (
              <ul className="edit__items">
                {(items?.items ?? []).map((it) => (
                  <ItemSummary
                    key={it.id}
                    item={it}
                    templates={publishable}
                    onEdit={() => setEditing(it.id)}
                    selected={editing === it.id}
                  />
                ))}
              </ul>
            )}
          </div>

          {editing ? (
            <ItemEditor
              key={editing}
              itemId={editing === 'new' ? null : editing}
              item={editing === 'new' ? null : items?.items.find((i) => i.id === editing) ?? null}
              templates={publishable}
              onDone={() => setEditing(null)}
            />
          ) : null}
        </div>
      )}
    </>
  )
}

function ItemSummary({
  item,
  templates,
  onEdit,
  selected,
}: {
  item: Item
  templates: TemplateSummary[]
  onEdit: () => void
  selected: boolean
}) {
  const readiness = useTopic<{ ready: boolean; missing: string[] }>(topics.itemReadiness(item.id))
  const template = templates.find((t) => t.id === item.template_id)
  const behind = template?.head_rev && template.head_rev !== item.template_rev

  return (
    <li className={selected ? 'edit__item edit__item--selected' : 'edit__item'}>
      <button type="button" className="edit__item-btn" onClick={onEdit}>
        <span className="edit__item-label">{item.label}</span>
        <span className="show__dim">{template?.name ?? 'unknown template'}</span>
      </button>
      <div className="edit__item-flags">
        {behind ? (
          // Pinned, on purpose. Saying it here is what makes the pinning a
          // feature rather than a surprise.
          <span className="edit__flag edit__flag--info">using an older version of this template</span>
        ) : null}
        {readiness && !readiness.ready ? (
          <span className="edit__flag edit__flag--warn">
            <AlertTriangle size={12} aria-hidden /> not ready on the node
          </span>
        ) : null}
      </div>
    </li>
  )
}

// ---------------------------------------------------------------------------

function ItemEditor({
  itemId,
  item,
  templates,
  onDone,
}: {
  itemId: string | null
  item: Item | null
  templates: TemplateSummary[]
  onDone: () => void
}) {
  const [templateId, setTemplateId] = useState(item?.template_id ?? templates[0]?.id ?? '')
  const { data: detail } = useQuery<TemplateDetail>(templateId ? keys.template(templateId) : null)
  const fields = detail?.head?.manifest?.fields ?? []

  const [label, setLabel] = useState(item?.label ?? '')
  const [data, setData] = useState<Record<string, string>>(
    Object.fromEntries(Object.entries(item?.data ?? {}).map(([k, v]) => [k, String(v)])),
  )
  const [layer, setLayer] = useState(item?.destination?.layer ?? 'Lower Third')
  const [problems, setProblems] = useState<string[]>([])
  const [saving, setSaving] = useState(false)

  // Prefill from the template's sample values on a NEW item only. Samples are
  // what Design previews with, so a new item starts looking like the thing the
  // designer intended rather than empty.
  useEffect(() => {
    if (item || fields.length === 0) return
    setData((cur) =>
      Object.keys(cur).length
        ? cur
        : Object.fromEntries(fields.filter((f) => f.sample != null).map((f) => [f.key, String(f.sample)])),
    )
  }, [fields, item])

  const save = useCallback(async () => {
    setSaving(true)
    setProblems([])
    try {
      const body = {
        template_id: templateId,
        label: label || undefined,
        data: coerce(fields, data),
        destination: { layer },
      }
      if (itemId) await patch(keys.item(itemId), body)
      else await post(keys.items(), body)
      invalidate(keys.items())
      onDone()
    } catch (e) {
      // The server validated against the PINNED revision's schema and answered
      // with sentences. Show all of them, not just the first — an operator
      // fixing one field at a time and resubmitting is a bad Thursday.
      if (e instanceof ApiError) {
        const detailProblems = (e.detail as { problems?: string[] } | undefined)?.problems
        setProblems(detailProblems ?? [e.message])
      } else {
        setProblems([String(e)])
      }
    } finally {
      setSaving(false)
    }
  }, [templateId, label, data, layer, fields, itemId, onDone])

  return (
    <form
      className="edit__editor"
      onSubmit={(e) => {
        e.preventDefault()
        void save()
      }}
    >
      <h2>{itemId ? 'Edit item' : 'New item'}</h2>

      <label className="field">
        <span>Template</span>
        <select value={templateId} onChange={(e) => setTemplateId(e.target.value)} disabled={Boolean(itemId)}>
          {templates.map((t) => (
            <option key={t.id} value={t.id}>{t.name}</option>
          ))}
        </select>
      </label>

      <label className="field">
        <span>Name in the rundown</span>
        <input value={label} onChange={(e) => setLabel(e.target.value)} placeholder="Pastor Dan" />
      </label>

      {fields.map((f) => (
        <FieldInput
          key={f.key}
          field={f}
          value={data[f.key] ?? ''}
          onChange={(v) => setData((d) => ({ ...d, [f.key]: v }))}
        />
      ))}

      <label className="field">
        <span>Layer</span>
        <input value={layer} onChange={(e) => setLayer(e.target.value)} />
      </label>

      {problems.length > 0 ? (
        <ul className="edit__problems" role="alert">
          {problems.map((p) => (
            <li key={p}>{p}</li>
          ))}
        </ul>
      ) : null}

      <div className="edit__actions">
        <button type="submit" className="btn btn--take" disabled={saving}>
          <Check size={15} aria-hidden /> {saving ? 'Saving…' : 'Save'}
        </button>
        <button type="button" className="btn btn--ghost" onClick={onDone}>
          Cancel
        </button>
      </div>
    </form>
  )
}

function FieldInput({
  field,
  value,
  onChange,
}: {
  field: Field
  value: string
  onChange: (v: string) => void
}) {
  // Count code points, matching the server: `max_len` is about what fits in a
  // box, and an emoji is one character to a human but two UTF-16 units.
  const used = [...value].length
  const over = field.max_len != null && used > field.max_len

  return (
    <label className="field">
      <span>
        {field.label ?? field.key}
        {field.required ? <em className="field__req"> required</em> : null}
        {field.max_len != null ? (
          <em className={over ? 'field__count field__count--over' : 'field__count'}>
            {used}/{field.max_len}
          </em>
        ) : null}
      </span>
      {field.type === 'bool' ? (
        <input type="checkbox" checked={value === 'true'} onChange={(e) => onChange(String(e.target.checked))} />
      ) : field.type === 'enum' ? (
        <select value={value} onChange={(e) => onChange(e.target.value)}>
          <option value="">—</option>
          {(field.options ?? []).map((o) => (
            <option key={o} value={o}>{o}</option>
          ))}
        </select>
      ) : (
        <input
          type={field.type === 'number' ? 'number' : field.type === 'color' ? 'color' : 'text'}
          value={value}
          onChange={(e) => onChange(e.target.value)}
          placeholder={field.sample}
        />
      )}
    </label>
  )
}

/** Send each field as the type its schema declares, not as whatever the DOM gave. */
function coerce(fields: Field[], data: Record<string, string>) {
  const out: Record<string, unknown> = {}
  for (const f of fields) {
    const raw = data[f.key]
    if (raw == null || raw === '') continue
    if (f.type === 'number') out[f.key] = Number(raw)
    else if (f.type === 'bool') out[f.key] = raw === 'true'
    else out[f.key] = raw
  }
  return out
}
