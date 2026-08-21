/**
 * Edit — for the person preparing this week's service.
 *
 * The console layout (ADR 0020): playlists and the rundown in the left rail,
 * the selected list's items in the centre, the item being edited in the right
 * inspector. Items, data, Playlists, Rundown order, destinations, readiness —
 * and nothing that belongs to Design or Show (ADR 0015).
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
 *
 * # Reordering is a whole-list write
 *
 * Move an entry and the entire list is sent back. Per-position edits from two
 * operators dragging at once produce an order neither of them chose; replacing
 * the list wholesale means the second write wins cleanly and *visibly*.
 *
 * # No drag-and-drop
 *
 * Up and down buttons, on purpose. jsdom has no layout, so a simulated drag
 * certifies nothing — a mode whose only path to reordering is a drag is a mode
 * whose reordering is untested. Drag can be added later *alongside* these.
 */

import { useCallback, useEffect, useState } from 'react'
import { AlertTriangle, ChevronDown, ChevronUp, FolderPlus, ListOrdered, Plus, X } from 'lucide-react'
import { useQuery, invalidate } from '../../lib/useQuery'
import { useTopic } from '../../lib/stream'
import { keys, topics } from '../../lib/keys'
import { ApiError, patch, post, put } from '../../lib/api'
import {
  Button,
  FormGroup,
  GroupHeader,
  NavItem,
  Panel,
  PanelBody,
  PanelHeader,
  StatusDot,
  StatusPill,
  Tag,
} from '../../components/console'
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

/** The timer declaration an Item stores — mirrors `pmg_scene::timer::TimerSpec`. */
type TimerSpec = {
  kind: 'clock' | 'countdown' | 'countup' | 'elapsed'
  at?: number
  format: 'hh_mm_ss' | 'h_mm_ss' | 'mm_ss' | 'm_ss' | 'hh_mm' | 'h_mm' | 'h_mm_am_pm'
}

type TemplateSummary = { id: string; name: string; head_rev: string | null; revisions: number }
type TemplateDetail = { id: string; name: string; head_rev: string | null; head: { manifest: { fields: Field[] } } | null }
type PlaylistSummary = { id: string; name: string; mode: string; items: number }
type PlaylistDetail = {
  id: string
  name: string
  mode: string
  items: (Item & { section?: string | null })[]
}
type RundownDetail = { id: string; name: string; playlists: PlaylistDetail[] }

/** Move the entry at `index` by `delta` and return the new order. */
function moved<T>(list: T[], index: number, delta: number): T[] {
  const to = index + delta
  if (to < 0 || to >= list.length) return list
  const copy = [...list]
  const [entry] = copy.splice(index, 1)
  copy.splice(to, 0, entry as T)
  return copy
}

export function Edit() {
  const { data: templates } = useQuery<{ templates: TemplateSummary[] }>(keys.templates())
  const { data: items } = useQuery<{ items: Item[] }>(keys.items())
  const { data: playlists } = useQuery<{ playlists: PlaylistSummary[] }>(keys.playlists())

  const [listSel, setListSel] = useState<string | 'all'>('all')
  const [editing, setEditing] = useState<string | 'new' | null>(null)

  const publishable = (templates?.templates ?? []).filter((t) => t.head_rev)
  const all = items?.items ?? []
  const lists = playlists?.playlists ?? []
  const currentList = listSel !== 'all' && lists.some((p) => p.id === listSel) ? listSel : 'all'

  const addPlaylist = useCallback(async () => {
    const p = await post<PlaylistSummary>(keys.playlists(), {
      name: `Playlist ${lists.length + 1}`,
    })
    invalidate(keys.playlists())
    setListSel(p.id)
  }, [lists.length])

  if (publishable.length === 0) {
    return (
      <div className="mode-scroll">
        <div className="mode__stub">
          <p>No template has been published yet.</p>
          <p>Build one in Design and publish it, then items can be built from it here.</p>
        </div>
      </div>
    )
  }

  return (
    <div className="edit-console">
      <div className="console__toolbar">
        <div className="console__toolbar-group">
          <Button onClick={() => setEditing('new')}>
            <Plus size={14} aria-hidden /> New item
          </Button>
          <span className="console__toolbar-note">
            Changes reach the node when you save. Readiness is checked per destination.
          </span>
        </div>
        <Tag tone="raised" size={10}>{`${publishable.length} templates published`}</Tag>
      </div>

      <div className="edit-console__panels">
        <Panel width="var(--pm-rail-width-edit)" label="Playlists">
          <PanelHeader
            title="Playlists"
            trailing={
              <button
                type="button"
                className="edit-console__iconbtn"
                aria-label="New playlist"
                onClick={() => void addPlaylist()}
              >
                <FolderPlus size={14} aria-hidden />
              </button>
            }
          />
          <PanelBody>
            <NavItem
              title="All items"
              meta={`${all.length} items`}
              icon={<ListOrdered size={16} aria-hidden />}
              active={currentList === 'all'}
              onClick={() => setListSel('all')}
            />
            {lists.map((p) => (
              <NavItem
                key={p.id}
                title={p.name}
                meta={`${p.items} items  ·  ${p.mode}`}
                active={currentList === p.id}
                onClick={() => setListSel(p.id)}
              />
            ))}
            <RundownRail playlists={lists} />
          </PanelBody>
        </Panel>

        <div className="console__center edit-console__center">
          {currentList === 'all' ? (
            <AllItems
              items={all}
              templates={publishable}
              editing={editing}
              onEdit={setEditing}
            />
          ) : (
            <PlaylistView
              id={currentList}
              allItems={all}
              templates={publishable}
              editing={editing}
              onEdit={setEditing}
            />
          )}
        </div>

        <Panel width="var(--pm-inspector-width)" label="Item">
          <PanelHeader
            title={editing === 'new' ? 'New item' : editing ? 'Edit item' : 'Item'}
            trailing={
              editing ? (
                <button
                  type="button"
                  className="edit-console__iconbtn"
                  aria-label="Close editor"
                  onClick={() => setEditing(null)}
                >
                  <X size={14} aria-hidden />
                </button>
              ) : undefined
            }
          />
          <PanelBody pad16>
            {editing ? (
              <ItemEditor
                key={editing}
                itemId={editing === 'new' ? null : editing}
                item={editing === 'new' ? null : (all.find((i) => i.id === editing) ?? null)}
                templates={publishable}
                onDone={() => setEditing(null)}
              />
            ) : (
              <p className="edit-console__dim">
                Select an item to edit it, or start a new one. The fields live once on the Item;
                each destination renders them with its own template.
              </p>
            )}
          </PanelBody>
        </Panel>
      </div>
    </div>
  )
}

// ---------------------------------------------------------------------------
// The rail's rundown block: which playlists make the show, in what order.

function RundownRail({ playlists }: { playlists: PlaylistSummary[] }) {
  const { data: rundowns } = useQuery<{ rundowns: { id: string; name: string }[] }>(keys.rundowns())
  const rundownId = rundowns?.rundowns?.[0]?.id ?? null
  const { data: rundown } = useQuery<RundownDetail>(rundownId ? keys.rundown(rundownId) : null)

  const addRundown = useCallback(async () => {
    await post(keys.rundowns(), { name: 'Sunday' })
    invalidate(keys.rundowns())
  }, [])

  const save = useCallback(
    async (ids: string[]) => {
      if (!rundownId) return
      await put(`${keys.rundown(rundownId)}/playlists`, { playlists: ids })
      invalidate(keys.rundown(rundownId))
      invalidate(keys.rundowns())
    },
    [rundownId],
  )

  const contents = rundown?.playlists ?? []
  const inRundown = new Set(contents.map((p) => p.id))

  return (
    <div className="edit-console__rundown">
      <GroupHeader
        label={rundown ? `Rundown · ${rundown.name}` : 'Rundown'}
        trailing={rundown ? `${contents.length} playlists` : undefined}
      />
      {!rundownId ? (
        <Button variant="secondary" block onClick={() => void addRundown()}>
          <Plus size={13} aria-hidden /> New rundown
        </Button>
      ) : (
        <>
          {contents.map((p, i) => (
            <div key={p.id} className="edit-console__order-row">
              <span className="edit-console__order-pos">{String(i + 1).padStart(2, '0')}</span>
              <span className="edit-console__order-label">{p.name}</span>
              <button
                type="button"
                className="edit-console__iconbtn"
                aria-label={`Move ${p.name} up`}
                disabled={i === 0}
                onClick={() => void save(moved(contents, i, -1).map((x) => x.id))}
              >
                <ChevronUp size={13} aria-hidden />
              </button>
              <button
                type="button"
                className="edit-console__iconbtn"
                aria-label={`Move ${p.name} down`}
                disabled={i === contents.length - 1}
                onClick={() => void save(moved(contents, i, 1).map((x) => x.id))}
              >
                <ChevronDown size={13} aria-hidden />
              </button>
              <button
                type="button"
                className="edit-console__iconbtn"
                aria-label={`Remove ${p.name}`}
                onClick={() => void save(contents.filter((x) => x.id !== p.id).map((x) => x.id))}
              >
                <X size={13} aria-hidden />
              </button>
            </div>
          ))}
          {playlists
            .filter((p) => !inRundown.has(p.id))
            .map((p) => (
              <button
                key={p.id}
                type="button"
                className="edit-console__addbtn"
                onClick={() => void save([...contents.map((x) => x.id), p.id])}
              >
                <Plus size={12} aria-hidden /> {p.name}
              </button>
            ))}
        </>
      )}
    </div>
  )
}

// ---------------------------------------------------------------------------
// Centre: every item, or one playlist's ordered entries.

function AllItems({
  items,
  templates,
  editing,
  onEdit,
}: {
  items: Item[]
  templates: TemplateSummary[]
  editing: string | 'new' | null
  onEdit: (id: string) => void
}) {
  return (
    <div className="edit-console__list">
      <div className="edit-console__list-head">
        <span className="edit-console__list-title">All items</span>
        <span className="edit-console__dim">
          An Item may carry several destinations. The Take fires every ready destination at once.
        </span>
      </div>
      {items.length === 0 ? (
        <p className="edit-console__dim">Nothing yet.</p>
      ) : (
        items.map((it, i) => (
          <ItemRow
            key={it.id}
            item={it}
            index={String(i + 1).padStart(2, '0')}
            templates={templates}
            selected={editing === it.id}
            onClick={() => onEdit(it.id)}
          />
        ))
      )}
    </div>
  )
}

function PlaylistView({
  id,
  allItems,
  templates,
  editing,
  onEdit,
}: {
  id: string
  allItems: Item[]
  templates: TemplateSummary[]
  editing: string | 'new' | null
  onEdit: (id: string) => void
}) {
  const { data: playlist } = useQuery<PlaylistDetail>(keys.playlist(id))
  const contents = playlist?.items ?? []
  const inList = new Set(contents.map((i) => i.id))

  const save = useCallback(
    async (entries: { item: string; section?: string | null }[]) => {
      await put(`${keys.playlist(id)}/items`, { items: entries })
      invalidate(keys.playlist(id))
      invalidate(keys.playlists())
      invalidate(keys.rundowns())
    },
    [id],
  )

  const entryOf = (it: Item & { section?: string | null }) => ({
    item: it.id,
    section: it.section ?? null,
  })

  const setMode = useCallback(
    async (mode: string) => {
      await put(`${keys.playlist(id)}/mode`, { mode })
      invalidate(keys.playlist(id))
      invalidate(keys.playlists())
      invalidate(keys.rundowns())
    },
    [id],
  )

  if (!playlist) return null

  return (
    <div className="edit-console__list">
      <div className="edit-console__list-head">
        <span className="edit-console__list-title">{playlist.name}</span>
        <Tag size={10} tone={playlist.mode === 'follow' ? 'armed' : 'quiet'}>
          {playlist.mode}
        </Tag>
      </div>

      {/* Its own control rather than a field that rides along with a rename:
          this changes what the operator's arrow keys DO (ADR 0017). Authored
          HERE because mode is a property of the content — Show only ever
          indicates it. */}
      <div className="edit-console__mode">
        <FormGroup label="Playout mode">
          <select value={playlist.mode} onChange={(e) => void setMode(e.target.value)}>
            <option value="cue">Cue — arm a row, then take it</option>
            <option value="follow">Follow — navigating is taking</option>
          </select>
        </FormGroup>
        <span className="edit-console__dim">
          {playlist.mode === 'follow'
            ? 'In Show, clicking or arrowing to an entry puts it on air. For songs and scripture.'
            : 'In Show, the arrows arm and taking is a separate act. For cued graphics.'}
        </span>
      </div>

      {contents.length === 0 ? (
        <p className="edit-console__dim">Empty. Add items from the pool below.</p>
      ) : (
        contents.map((it, i) => (
          <div key={it.id} className="edit-console__entry">
            <ItemRow
              item={it}
              index={String(i + 1).padStart(2, '0')}
              templates={templates}
              selected={editing === it.id}
              onClick={() => onEdit(it.id)}
            />
            <div className="edit-console__entry-side">
              <input
                className="edit-console__section"
                placeholder="Section"
                aria-label={`Section for ${it.label}`}
                defaultValue={it.section ?? ''}
                onBlur={(e) => {
                  const section = e.target.value.trim() || null
                  if ((it.section ?? null) === section) return
                  void save(contents.map((x, n) => (n === i ? { item: x.id, section } : entryOf(x))))
                }}
              />
              <div className="edit-console__entry-btns">
                <button
                  type="button"
                  className="edit-console__iconbtn"
                  aria-label={`Move ${it.label} up`}
                  disabled={i === 0}
                  onClick={() => void save(moved(contents, i, -1).map(entryOf))}
                >
                  <ChevronUp size={13} aria-hidden />
                </button>
                <button
                  type="button"
                  className="edit-console__iconbtn"
                  aria-label={`Move ${it.label} down`}
                  disabled={i === contents.length - 1}
                  onClick={() => void save(moved(contents, i, 1).map(entryOf))}
                >
                  <ChevronDown size={13} aria-hidden />
                </button>
                <button
                  type="button"
                  className="edit-console__iconbtn"
                  aria-label={`Remove ${it.label}`}
                  onClick={() => void save(contents.filter((x) => x.id !== it.id).map(entryOf))}
                >
                  <X size={13} aria-hidden />
                </button>
              </div>
            </div>
          </div>
        ))
      )}

      {allItems.filter((it) => !inList.has(it.id)).length ? (
        <div className="edit-console__pool">
          <GroupHeader label="Add to this playlist" />
          <div className="edit-console__pool-btns">
            {allItems
              .filter((it) => !inList.has(it.id))
              .map((it) => (
                <button
                  key={it.id}
                  type="button"
                  className="edit-console__addbtn"
                  onClick={() => void save([...contents.map(entryOf), { item: it.id }])}
                >
                  <Plus size={12} aria-hidden /> {it.label}
                </button>
              ))}
          </div>
        </div>
      ) : null}
    </div>
  )
}

/**
 * One item, the stacked treatment: every destination readable without opening
 * the Item. Costs row height, keeps the fan-out visible at the moment it
 * matters — a count would hide exactly the destination that is excluded.
 */
function ItemRow({
  item,
  index,
  templates,
  selected,
  onClick,
}: {
  item: Item
  index: string
  templates: TemplateSummary[]
  selected: boolean
  onClick: () => void
}) {
  const readiness = useTopic<{ ready: boolean; missing: string[] }>(topics.itemReadiness(item.id))
  const template = templates.find((t) => t.id === item.template_id)
  const behind = template?.head_rev && template.head_rev !== item.template_rev
  const dests = item.destinations ?? []

  return (
    <button
      type="button"
      className={selected ? 'edit-console__item edit-console__item--selected' : 'edit-console__item'}
      onClick={onClick}
    >
      <span className="edit-console__item-index">{index}</span>
      <span className="edit-console__item-body">
        <span className="edit-console__item-title">{item.label}</span>
        <span className="edit-console__item-kind">{template?.name ?? 'unknown template'}</span>
        {dests.map((d, n) => (
          <span key={n} className="edit-console__item-dest">
            <StatusDot tone={d.blank ? 'muted' : 'ready'} size={6} />
            <span className="edit-console__item-route">
              {d.output || 'any output'}  ·  {d.layer}
            </span>
            <span className="edit-console__item-destnote">
              {d.blank ? 'blanks this feed' : d.mode === 'on_request' ? 'only when asked for' : ''}
            </span>
          </span>
        ))}
      </span>
      <span className="edit-console__item-flags">
        {dests.length > 1 ? (
          <StatusPill tone="armed" variant="subtle">{`${dests.length} DESTINATIONS`}</StatusPill>
        ) : null}
        {behind ? (
          // Pinned, on purpose. Saying it here is what makes the pinning a
          // feature rather than a surprise.
          <span className="edit-console__flag">older template version</span>
        ) : null}
        {readiness && !readiness.ready ? (
          <span className="edit-console__flag edit-console__flag--warn">
            <AlertTriangle size={12} aria-hidden /> not ready on the node
          </span>
        ) : null}
      </span>
    </button>
  )
}

// ---------------------------------------------------------------------------
// The item inspector — the authoring half of ADR 0018.

/**
 * Where an Item goes.
 *
 * An Item is one set of words that appears in several places, each rendered by
 * its own template: a narrow strip over the key feed, full-bleed type on the
 * LED wall.
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
 * There are deliberately no per-destination data overrides (ADR 0018): two
 * values for one field re-opens "which one is the real Item".
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
  const set = (i: number, patchd: Partial<DestinationDraft>) =>
    onChange(value.map((d, n) => (n === i ? { ...d, ...patchd } : d)))

  return (
    <div className="edit-console__dests">
      <div className="edit-console__dests-head">
        <GroupHeader label="Where it goes" />
        <button
          type="button"
          className="edit-console__addbtn"
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
          <div key={i} className="edit-console__dest-card">
            <div className="edit-console__dest-row">
              <FormGroup label="Output">
                <select value={d.output} onChange={(e) => set(i, { output: e.target.value, layer: '' })}>
                  <option value="">Whichever node has it</option>
                  {outputs.map((o) => (
                    <option key={o.output_id} value={o.output_id}>{o.output_id}</option>
                  ))}
                </select>
              </FormGroup>
              <FormGroup label="Layer">
                {/* By NAME. An index silently re-points at a different graphic
                    the moment someone reorders the stack. */}
                <select value={d.layer} onChange={(e) => set(i, { layer: e.target.value })}>
                  <option value="">Choose a layer</option>
                  {layers.map((l) => <option key={l} value={l}>{l}</option>)}
                  {d.layer && !layers.includes(d.layer) ? (
                    <option value={d.layer}>{d.layer} (not on this output)</option>
                  ) : null}
                </select>
              </FormGroup>
              {value.length > 1 ? (
                <button
                  type="button"
                  className="edit-console__iconbtn edit-console__dest-remove"
                  aria-label={`Remove destination ${i + 1}`}
                  onClick={() => onChange(value.filter((_, n) => n !== i))}
                >
                  <X size={13} aria-hidden />
                </button>
              ) : null}
            </div>

            <div className="edit-console__dest-row">
              <FormGroup label="Rendered by">
                <select
                  value={d.template_id}
                  disabled={d.blank}
                  onChange={(e) => set(i, { template_id: e.target.value })}
                >
                  {templates.map((t) => <option key={t.id} value={t.id}>{t.name}</option>)}
                </select>
              </FormGroup>
              <FormGroup label="When">
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
              </FormGroup>
            </div>

            {i === 0 && value.length > 1 ? (
              <p className="edit-console__hint">
                This one’s template decides which fields you fill in above. The others are checked
                against them when you save.
              </p>
            ) : null}
            {d.blank ? (
              <p className="edit-console__hint">
                Taking this item will <b>clear</b> this feed. That is an instruction, not a gap —
                it is how a full-screen graphic blanks the wall.
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
  // Form state is strings, one shape for every field. A timer's value is the
  // one object in an Item's data, so it rides as its own JSON and `coerce`
  // parses it back on save — `String(v)` on it would store the literal text
  // "[object Object]", which is exactly the sort of thing that gets found on a
  // Sunday.
  const [data, setData] = useState<Record<string, string>>(
    Object.fromEntries(
      Object.entries(item?.data ?? {}).map(([k, v]) => [
        k,
        v !== null && typeof v === 'object' ? JSON.stringify(v) : String(v),
      ]),
    ),
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
      className="edit-console__editor"
      onSubmit={(e) => {
        e.preventDefault()
        void save()
      }}
    >
      <FormGroup label="Name in the rundown">
        <input value={label} onChange={(e) => setLabel(e.target.value)} placeholder="Pastor Dan" />
      </FormGroup>

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
        <ul className="edit-console__problems" role="alert">
          {problems.map((p) => (
            <li key={p}>{p}</li>
          ))}
        </ul>
      ) : null}

      <div className="edit-console__actions">
        <button type="submit" className="c-btn c-btn--primary" disabled={saving}>
          {saving ? 'Saving…' : 'Save'}
        </button>
        <Button variant="secondary" onClick={onDone}>
          Cancel
        </Button>
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

  const label = [
    field.label ?? field.key,
    field.required ? '· required' : '',
    field.max_len != null ? `· ${used}/${field.max_len}${over ? ' — too long' : ''}` : '',
  ]
    .filter(Boolean)
    .join(' ')

  return (
    <FormGroup label={label}>
      {field.type === 'image' || field.type === 'video' ? (
        // The Item's own media, picked from what this server holds. The value
        // is the content address — the same identity the node stages by, so
        // choosing here IS choosing what plays.
        <MediaPicker kind={field.type} value={value} onChange={onChange} />
      ) : field.type === 'timer' ? (
        <TimerInput value={value} onChange={onChange} />
      ) : field.type === 'bool' ? (
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
    </FormGroup>
  )
}

/**
 * A timer, which is a DECLARATION rather than a value (ADR 0022).
 *
 * The node computes the number from its own clock, so what an operator sets
 * here is what it counts and how it reads — never the number itself. The form
 * state stays a JSON string like every other field's, and `coerce` parses it on
 * the way out; that keeps one shape for the whole Item.
 */
function TimerInput({ value, onChange }: { value: string; onChange: (v: string) => void }) {
  const spec = (() => {
    try {
      const p = JSON.parse(value || '{}') as { timer?: TimerSpec }
      return p.timer ?? null
    } catch {
      return null
    }
  })()
  const kind = spec?.kind ?? 'countdown'
  const format = spec?.format ?? 'm_ss'
  const at = spec?.at ?? null

  const set = (next: Partial<TimerSpec>) => {
    const merged: TimerSpec = { kind, format, ...(at != null ? { at } : {}), ...next }
    // A clock and an elapsed timer count from something other than an instant,
    // so carrying one would be a field nobody set and nobody can see.
    if (merged.kind === 'clock' || merged.kind === 'elapsed') delete merged.at
    onChange(JSON.stringify({ timer: merged }))
  }

  // <input type="datetime-local"> speaks local time with no zone; the
  // declaration stores UTC milliseconds, so the two are converted here and
  // nowhere else.
  const localValue = at == null ? '' : new Date(at - new Date(at).getTimezoneOffset() * 60000).toISOString().slice(0, 16)

  return (
    <div className="timer">
      <select value={kind} aria-label="Timer kind" onChange={(e) => set({ kind: e.target.value as TimerSpec['kind'] })}>
        <option value="countdown">Counts down to a time</option>
        <option value="countup">Counts up from a time</option>
        <option value="clock">Shows the time of day</option>
        <option value="elapsed">Counts up from the take</option>
      </select>
      {kind === 'countdown' || kind === 'countup' ? (
        <input
          type="datetime-local"
          aria-label="Timer target time"
          value={localValue}
          onChange={(e) => {
            const ms = e.target.value ? new Date(e.target.value).getTime() : null
            set(ms != null && Number.isFinite(ms) ? { at: ms } : {})
          }}
        />
      ) : null}
      <select value={format} aria-label="Timer format" onChange={(e) => set({ format: e.target.value as TimerSpec['format'] })}>
        <option value="m_ss">4:32</option>
        <option value="mm_ss">04:32</option>
        <option value="h_mm_ss">1:04:32</option>
        <option value="hh_mm_ss">01:04:32</option>
        <option value="h_mm">10:05</option>
        <option value="hh_mm">10:05, padded</option>
        <option value="h_mm_am_pm">10:05 AM</option>
      </select>
      {kind === 'countdown' && at == null ? (
        <p className="edit-console__dim">
          Pick the time it counts to. Without one it counts to now and sits at zero, which
          looks exactly like a graphic that has broken.
        </p>
      ) : null}
    </div>
  )
}

/** A dropdown of this server's media assets of one kind, valued by hash. */
function MediaPicker({
  kind,
  value,
  onChange,
}: {
  kind: 'image' | 'video'
  value: string
  onChange: (v: string) => void
}) {
  const { data } = useQuery<{ assets: { hash: string; size: number; kind: string }[] }>(keys.assets())
  const options = (data?.assets ?? []).filter((a) => a.kind === kind)
  const short = (h: string) => h.replace(/^[a-z0-9]+:/, '').slice(0, 8)
  return (
    <select value={value} onChange={(e) => onChange(e.target.value)}>
      <option value="">—</option>
      {value && !options.some((a) => a.hash === value) ? (
        <option value={value}>{short(value)} — not on this server</option>
      ) : null}
      {options.map((a) => (
        <option key={a.hash} value={a.hash}>
          {short(a.hash)} · {Math.max(1, Math.round(a.size / 1024))} KB
        </option>
      ))}
    </select>
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
    else if (f.type === 'timer') {
      // The one field whose value is an object. A malformed one is dropped
      // rather than sent: the server would refuse it anyway, and refusing here
      // means the message names the field instead of the JSON.
      try {
        out[f.key] = JSON.parse(raw) as unknown
      } catch {
        continue
      }
    } else out[f.key] = raw
  }
  return out
}
