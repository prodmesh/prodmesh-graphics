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
import { AlertTriangle, Check, Plus, X } from 'lucide-react'
import { useQuery, invalidate } from '../../lib/useQuery'
import { useTopic } from '../../lib/stream'
import { keys, topics } from '../../lib/keys'
import { ApiError, patch, post } from '../../lib/api'
import { Running } from './Running'
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

      {publishable.length > 0 ? <Running items={items?.items ?? []} /> : null}
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


/**
 * Where an Item goes — the authoring half of ADR 0018.
 *
 * An Item is one set of words that appears in several places, each rendered by
 * its own template: a narrow strip over the key feed, full-bleed type on the
 * LED wall. Until now the model could express that and the app could not, so
 * the only way to build one was a script against the API.
 *
 * # Three things a destination can be, and they are not degrees of the same one
 *
 * **Fires with the take** is the default. **Only when asked for** is the
 * full-screen version of a lower third — available, and deliberately skipped
 * by a take, so it needs the separate "Also full screen" action in Show.
 * **Render nothing** is an instruction: a full-screen graphic that clears the
 * LED wall is *saying* to clear it, which is different from a feed that failed
 * and different again from one nobody configured.
 *
 * # The field list belongs to the first destination
 *
 * The Item carries one set of data and the server checks it against EVERY
 * destination's template, so a second destination whose template wants a field
 * the Item does not have is a mistake made now and discovered on Sunday. The
 * first destination's template defines the fields shown; the others are
 * checked against them on save and the server's sentence is rendered verbatim.
 */
function Destinations({
  value,
  templates,
  outputs,
  onChange,
}: {
  value: DestinationDraft[]
  templates: TemplateSummary[]
  outputs: { output_id: string; layers: string[] }[]
  onChange: (next: DestinationDraft[]) => void
}) {
  const set = (i: number, patch: Partial<DestinationDraft>) =>
    onChange(value.map((d, n) => (n === i ? { ...d, ...patch } : d)))

  return (
    <div className="edit__dests">
      <div className="edit__list-head">
        <h3>Where it goes</h3>
        <button
          type="button"
          className="btn btn--ghost btn--small"
          onClick={() =>
            onChange([
              ...value,
              // A new one copies the first's template rather than defaulting to
              // whatever is alphabetically first: the common second destination
              // is the same graphic somewhere else, and a wrong template here
              // is a silent mistake.
              { ...(value[0] ?? blankDest(templates)), output: '', mode: 'auto', blank: false },
            ])
          }
        >
          <Plus size={13} aria-hidden /> Add a destination
        </button>
      </div>

      {value.map((d, i) => {
        const layers = outputs.find((o) => o.output_id === d.output)?.layers ?? []
        return (
          <div key={i} className="edit__dest">
            <div className="edit__dest-row">
              <label>
                <span>Output</span>
                <select value={d.output} onChange={(e) => set(i, { output: e.target.value, layer: '' })}>
                  <option value="">Whichever node has it</option>
                  {outputs.map((o) => (
                    <option key={o.output_id} value={o.output_id}>{o.output_id}</option>
                  ))}
                </select>
              </label>
              <label>
                <span>Layer</span>
                {/* By NAME. An index silently re-points at a different graphic
                    the moment someone reorders the stack. */}
                <select value={d.layer} onChange={(e) => set(i, { layer: e.target.value })}>
                  <option value="">Choose a layer</option>
                  {layers.map((l) => <option key={l} value={l}>{l}</option>)}
                  {d.layer && !layers.includes(d.layer) ? (
                    <option value={d.layer}>{d.layer} (not on this output)</option>
                  ) : null}
                </select>
              </label>
              {value.length > 1 ? (
                <button
                  type="button"
                  className="btn btn--ghost btn--small"
                  aria-label={`Remove destination ${i + 1}`}
                  onClick={() => onChange(value.filter((_, n) => n !== i))}
                >
                  <X size={13} aria-hidden />
                </button>
              ) : null}
            </div>

            <div className="edit__dest-row">
              <label>
                <span>Rendered by</span>
                <select
                  value={d.template_id}
                  disabled={d.blank}
                  onChange={(e) => set(i, { template_id: e.target.value })}
                >
                  {templates.map((t) => <option key={t.id} value={t.id}>{t.name}</option>)}
                </select>
              </label>
              <label>
                <span>When</span>
                <select
                  value={d.blank ? 'blank' : d.mode}
                  onChange={(e) => {
                    const v = e.target.value
                    set(i, v === 'blank'
                      ? { blank: true, mode: 'auto' }
                      : { blank: false, mode: v })
                  }}
                >
                  <option value="auto">Fires with the take</option>
                  <option value="on_request">Only when asked for</option>
                  <option value="blank">Render nothing — clear this feed</option>
                </select>
              </label>
            </div>

            {i === 0 && value.length > 1 ? (
              <p className="edit__hint">
                This one’s template decides which fields you fill in above. The
                others are checked against them when you save.
              </p>
            ) : null}
            {d.blank ? (
              <p className="edit__hint">
                Taking this item will <b>clear</b> this feed. That is an
                instruction, not a gap — it is how a full-screen graphic blanks
                the wall.
              </p>
            ) : null}
          </div>
        )
      })}
    </div>
  )
}

type DestinationDraft = {
  output: string
  layer: string
  template_id: string
  mode: string
  blank: boolean
}

const blankDest = (templates: TemplateSummary[]): DestinationDraft => ({
  output: '',
  layer: 'Lower Third',
  template_id: templates[0]?.id ?? '',
  mode: 'auto',
  blank: false,
})

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
  // The Item's destinations, as a draft. An Item made before ADR 0018 has one
  // implied destination, and it becomes a list of one rather than a special
  // case — the same shape the server's migration used.
  const [dests, setDests] = useState<DestinationDraft[]>(() => {
    const existing = item?.destinations ?? []
    if (existing.length) {
      return existing.map((d) => ({
        output: d.output ?? '',
        layer: d.layer ?? '',
        template_id:
          templates.find((t) => t.head_rev === d.template_rev)?.id ??
          item?.template_id ??
          templates[0]?.id ??
          '',
        mode: d.mode ?? 'auto',
        blank: !!d.blank,
      }))
    }
    return [{
      output: item?.destination?.output ?? '',
      layer: item?.destination?.layer ?? 'Lower Third',
      template_id: item?.template_id ?? templates[0]?.id ?? '',
      mode: 'auto',
      blank: false,
    }]
  })
  // The FIRST destination's template decides which fields the operator fills
  // in. The Item carries one set of data and the server checks it against every
  // destination, so one of them has to define the form — and the first is the
  // one an operator thinks of as "the" graphic.
  const templateId = dests[0]?.template_id || item?.template_id || templates[0]?.id || ''
  const { data: detail } = useQuery<TemplateDetail>(templateId ? keys.template(templateId) : null)
  const fields = detail?.head?.manifest?.fields ?? []

  const [label, setLabel] = useState(item?.label ?? '')
  const [data, setData] = useState<Record<string, string>>(
    Object.fromEntries(Object.entries(item?.data ?? {}).map(([k, v]) => [k, String(v)])),
  )
  // The outputs an operator can actually pick. Read from the node rather than
  // typed, because a layer name that does not exist on the output is a cue the
  // node will refuse — and finding that out at the take is the whole failure
  // this list prevents.
  const { data: nodes } = useQuery<{ nodes: { node_id: string }[] }>(keys.nodes())
  const nodeId = nodes?.nodes?.[0]?.node_id ?? null
  const { data: outs } = useQuery<{ outputs: { output_id: string; layers: string[] }[] }>(
    nodeId ? keys.outputs(nodeId) : null,
  )
  const outputs = outs?.outputs ?? []

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
        // Resolved to a REVISION here, because a destination pins the revision
        // it was built against — publishing a new one must not silently change
        // what an existing Item renders.
        destinations: dests.map((d) => ({
          output: d.output || undefined,
          layer: d.layer || undefined,
          template_rev: templates.find((t) => t.id === d.template_id)?.head_rev ?? undefined,
          mode: d.mode,
          blank: d.blank,
        })),
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
  }, [templateId, label, data, dests, templates, fields, itemId, onDone])

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

      <Destinations
        value={dests}
        templates={templates}
        outputs={outputs}
        onChange={setDests}
      />

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
