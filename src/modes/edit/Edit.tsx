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
import { AlertTriangle, ChevronDown, ChevronUp, Copy, CornerUpRight, FolderPlus, Pencil, Plus, Search, Trash2, X } from 'lucide-react'
import { useQuery, invalidate } from '../../lib/useQuery'
import { useTopic } from '../../lib/stream'
import { keys, topics } from '../../lib/keys'
import { ApiError, del, get, patch, post, put } from '../../lib/api'
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
type PlaylistSummary = { id: string; name: string; mode: string; items: number; tags?: string[] }
type Arrangement = { id: string; name: string; groups: string[] }
type PlaylistDetail = {
  id: string
  name: string
  mode: string
  tags?: string[]
  /** Which named order this reading is under. Null is Main (ADR 0027). */
  arrangement?: string | null
  arrangements?: Arrangement[]
  /** The groups Main defines, with how many slides each holds. */
  groups?: { name: string; slides: number }[]
  /** Slides in Main that no group contains, so no arrangement can reach them. */
  ungrouped?: string[]
  items: Entry[]
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

/** An Item at a place in a playlist. The entry identifies the place. */
type Entry = Item & { entry_id: string; section?: string | null }

export function Edit() {
  const { data: templates } = useQuery<{ templates: TemplateSummary[] }>(keys.templates())
  const { data: items } = useQuery<{ items: Item[] }>(keys.items())
  const { data: playlists } = useQuery<{ playlists: PlaylistSummary[] }>(keys.playlists())

  const [listSel, setListSel] = useState<string | 'find'>('find')
  const [editing, setEditing] = useState<string | 'new' | null>(null)
  const [filter, setFilter] = useState('')
  const [tag, setTag] = useState<string | null>(null)

  const publishable = (templates?.templates ?? []).filter((t) => t.head_rev)
  const all = items?.items ?? []
  const lists = playlists?.playlists ?? []
  const currentList = listSel !== 'find' && lists.some((p) => p.id === listSel) ? listSel : 'find'

  // A hundred songs is a hundred rows. Name match, then tag — both plain, both
  // instant, and neither of them a mode the operator has to leave.
  const needle = filter.trim().toLowerCase()
  const shown = lists.filter(
    (p) =>
      (!needle || p.name.toLowerCase().includes(needle)) &&
      (!tag || (p.tags ?? []).includes(tag)),
  )

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
          <Button
            disabled={currentList === 'find'}
            title={currentList === 'find' ? 'Open a playlist first — an item belongs to one' : undefined}
            onClick={() => setEditing('new')}
          >
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
            {/* "All items" used to live here. At a hundred songs of thirty
                slides each it is thousands of rows, which is not a way to find
                anything — so the way in is a search, and the rail is the
                library. */}
            <NavItem
              title="Find an item…"
              meta="across every playlist"
              icon={<Search size={16} aria-hidden />}
              active={currentList === 'find'}
              onClick={() => setListSel('find')}
            />

            <input
              className="edit-console__filter"
              value={filter}
              placeholder="Filter playlists"
              aria-label="Filter playlists"
              onChange={(e) => setFilter(e.target.value)}
            />
            <TagFilter picked={tag} onPick={setTag} />

            {shown.map((p) => (
              <NavItem
                key={p.id}
                title={p.name}
                meta={[`${p.items} items`, p.mode, ...(p.tags ?? [])].join('  ·  ')}
                active={currentList === p.id}
                onClick={() => setListSel(p.id)}
              />
            ))}
            {lists.length && !shown.length ? (
              <p className="edit-console__dim">Nothing matches. Clear the filter to see them all.</p>
            ) : null}
            <RundownRail playlists={lists} />
          </PanelBody>
        </Panel>

        <div className="console__center edit-console__center">
          {currentList === 'find' ? (
            <FindItems
              templates={publishable}
              editing={editing}
              onOpen={(itemId, playlistId) => {
                setListSel(playlistId)
                setEditing(itemId)
              }}
            />
          ) : (
            <PlaylistView
              id={currentList}
              templates={publishable}
              playlists={lists}
              editing={editing}
              onEdit={setEditing}
              onGo={setListSel}
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
                playlistId={currentList === 'find' ? null : currentList}
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

/**
 * Which playlists make the show, in what order — and WHICH show.
 *
 * It used to render `rundowns[0]` and nothing else: one running order, created
 * once, unnameable and unswitchable, with no way to start next week's without
 * editing this week's. A rundown is cheap and a building has several — this
 * Sunday, next Sunday, the Christmas Eve service — so choosing between them is
 * the normal case rather than an advanced one.
 */
function RundownRail({ playlists }: { playlists: PlaylistSummary[] }) {
  const { data: rundowns } = useQuery<{ rundowns: { id: string; name: string }[] }>(keys.rundowns())
  const all = rundowns?.rundowns ?? []
  const [picked, setPicked] = useState<string | null>(null)
  const [renaming, setRenaming] = useState(false)
  // A rundown that has been deleted must not leave the rail pointing at
  // nothing: fall back to the first rather than rendering an empty shell.
  const rundownId = picked && all.some((r) => r.id === picked) ? picked : (all[0]?.id ?? null)
  const { data: rundown } = useQuery<RundownDetail>(rundownId ? keys.rundown(rundownId) : null)

  const addRundown = useCallback(async () => {
    const made = await post<{ id: string }>(keys.rundowns(), {
      name: `Rundown ${(rundowns?.rundowns?.length ?? 0) + 1}`,
    })
    invalidate(keys.rundowns())
    setPicked(made.id)
    setRenaming(true)
  }, [rundowns])

  const save = useCallback(
    async (entries: { playlist: string; arrangement: string | null }[]) => {
      if (!rundownId) return
      await put(`${keys.rundown(rundownId)}/playlists`, { playlists: entries })
      invalidate(keys.rundown(rundownId))
      invalidate(keys.rundowns())
    },
    [rundownId],
  )

  /** This service's choice of order for each song (ADR 0027). */
  const pinned = (p: PlaylistDetail) => ({ playlist: p.id, arrangement: p.arrangement ?? null })

  const contents = rundown?.playlists ?? []
  const inRundown = new Set(contents.map((p) => p.id))

  return (
    <div className="edit-console__rundown">
      <GroupHeader
        label="Rundown"
        trailing={rundown ? `${contents.length} playlists` : undefined}
      />

      {all.length === 0 ? (
        <Button variant="secondary" block onClick={() => void addRundown()}>
          <Plus size={13} aria-hidden /> New rundown
        </Button>
      ) : (
        <>
          <div className="edit-console__rundown-pick">
            {renaming && rundown ? (
              <input
                className="edit-console__rename"
                autoFocus
                defaultValue={rundown.name}
                aria-label="Rundown name"
                onKeyDown={(e) => {
                  if (e.key === 'Enter') (e.target as HTMLInputElement).blur()
                  if (e.key === 'Escape') {
                    ;(e.target as HTMLInputElement).value = rundown.name
                    ;(e.target as HTMLInputElement).blur()
                  }
                }}
                onBlur={(e) => {
                  const name = e.target.value.trim()
                  setRenaming(false)
                  if (!name || name === rundown.name) return
                  void (async () => {
                    await patch(keys.rundown(rundown.id), { name })
                    invalidate(keys.rundowns())
                    invalidate(keys.rundown(rundown.id))
                  })()
                }}
              />
            ) : (
              <select
                value={rundownId ?? ''}
                aria-label="Rundown"
                onChange={(e) => setPicked(e.target.value)}
              >
                {all.map((r) => <option key={r.id} value={r.id}>{r.name}</option>)}
              </select>
            )}
            <button
              type="button"
              className="edit-console__iconbtn"
              aria-label="Rename this rundown"
              onClick={() => setRenaming(true)}
            >
              <Pencil size={13} aria-hidden />
            </button>
            <button
              type="button"
              className="edit-console__iconbtn"
              aria-label="New rundown"
              onClick={() => void addRundown()}
            >
              <Plus size={13} aria-hidden />
            </button>
            <button
              type="button"
              className="edit-console__iconbtn"
              aria-label="Delete this rundown"
              disabled={!rundownId}
              onClick={() => {
                if (!rundown) return
                // The playlists survive — a rundown is an order over them.
                if (!window.confirm(`Delete the rundown “${rundown.name}”? Its playlists stay.`)) return
                void (async () => {
                  await del(keys.rundown(rundown.id))
                  setPicked(null)
                  invalidate(keys.rundowns())
                })()
              }}
            >
              <Trash2 size={13} aria-hidden />
            </button>
          </div>

          {contents.map((p, i) => (
            <div key={p.id} className="edit-console__order-entry">
              <div className="edit-console__order-row">
                <span className="edit-console__order-pos">{String(i + 1).padStart(2, '0')}</span>
                <span className="edit-console__order-label">{p.name}</span>
                <button
                  type="button"
                  className="edit-console__iconbtn"
                  aria-label={`Move ${p.name} up`}
                  disabled={i === 0}
                  onClick={() => void save(moved(contents, i, -1).map(pinned))}
                >
                  <ChevronUp size={13} aria-hidden />
                </button>
                <button
                  type="button"
                  className="edit-console__iconbtn"
                  aria-label={`Move ${p.name} down`}
                  disabled={i === contents.length - 1}
                  onClick={() => void save(moved(contents, i, 1).map(pinned))}
                >
                  <ChevronDown size={13} aria-hidden />
                </button>
                <button
                  type="button"
                  className="edit-console__iconbtn"
                  aria-label={`Remove ${p.name}`}
                  onClick={() => void save(contents.filter((x) => x.id !== p.id).map(pinned))}
                >
                  <X size={13} aria-hidden />
                </button>
              </div>
              {/* Which order of this song THIS service wants. Shown only when
                  the song has more than one — a picker with one option is a
                  question with one answer (ADR 0027). */}
              {(p.arrangements ?? []).length ? (
                <select
                  className="edit-console__order-arr"
                  value={p.arrangement ?? ''}
                  aria-label={`Arrangement of ${p.name}`}
                  onChange={(e) =>
                    void save(
                      contents.map((x) =>
                        x.id === p.id
                          ? { playlist: x.id, arrangement: e.target.value || null }
                          : pinned(x),
                      ),
                    )
                  }
                >
                  <option value="">Main</option>
                  {(p.arrangements ?? []).map((a) => (
                    <option key={a.id} value={a.id}>{a.name}</option>
                  ))}
                </select>
              ) : null}
            </div>
          ))}
          {playlists
            .filter((p) => !inRundown.has(p.id))
            .map((p) => (
              <button
                key={p.id}
                type="button"
                className="edit-console__addbtn"
                onClick={() => void save([...contents.map(pinned), { playlist: p.id, arrangement: null }])}
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

/**
 * The tags anybody has typed, as a one-of filter.
 *
 * Free strings and no admin screen (ADR 0026): a vocabulary nobody can add to
 * is a vocabulary nobody uses, so the set on offer is simply what has been
 * typed on some playlist.
 */
function TagFilter({ picked, onPick }: { picked: string | null; onPick: (t: string | null) => void }) {
  const { data } = useQuery<{ tags: { tag: string; playlists: number }[] }>(keys.tags())
  const tags = data?.tags ?? []
  if (!tags.length) return null
  return (
    <div className="edit-console__tagfilter" role="group" aria-label="Filter by tag">
      {tags.map((t) => (
        <button
          key={t.tag}
          type="button"
          aria-pressed={picked === t.tag}
          className={
            picked === t.tag ? 'edit-console__tagchip edit-console__tagchip--on' : 'edit-console__tagchip'
          }
          onClick={() => onPick(picked === t.tag ? null : t.tag)}
        >
          {t.tag}
          <span>{t.playlists}</span>
        </button>
      ))}
    </div>
  )
}

/**
 * "Which song has that line?"
 *
 * This is what replaced the All items list. A permanent view of every Item is
 * thousands of rows at the scale this is being loaded to; a search costs
 * nothing when it is not being used and answers the question the list was
 * actually being scrolled for. The server searches labels and the values a
 * person typed, and names the playlist each result lives in.
 */
function FindItems({
  templates,
  editing,
  onOpen,
}: {
  templates: TemplateSummary[]
  editing: string | 'new' | null
  onOpen: (itemId: string, playlistId: string) => void
}) {
  const [q, setQ] = useState('')
  const [live, setLive] = useState('')
  useEffect(() => {
    // Typed, not per keystroke: a search that fires on every letter is a search
    // that races itself.
    const t = setTimeout(() => setLive(q.trim()), 200)
    return () => clearTimeout(t)
  }, [q])

  const { data } = useQuery<{ items: (Item & { playlist_id?: string; playlist_name?: string })[] }>(
    live.length >= 2 ? keys.itemSearch(live) : null,
  )
  const found = data?.items ?? []

  return (
    <div className="edit-console__list">
      <div className="edit-console__list-head">
        <span className="edit-console__list-title edit-console__list-title--plain">Find an item</span>
      </div>
      <input
        className="edit-console__search"
        autoFocus
        value={q}
        placeholder="Words in an item — its name, or anything typed into it"
        aria-label="Search items"
        onChange={(e) => setQ(e.target.value)}
      />
      {live.length < 2 ? (
        <p className="edit-console__dim">
          Type at least two letters. Every playlist is searched, and each result says which one it
          is in.
        </p>
      ) : found.length === 0 ? (
        <p className="edit-console__dim">Nothing matches “{live}”.</p>
      ) : (
        found.map((it) => (
          <button
            key={it.id}
            type="button"
            className="edit-console__found"
            onClick={() => it.playlist_id && onOpen(it.id, it.playlist_id)}
          >
            <ItemRow
              item={it}
              index={it.playlist_name ?? ''}
              templates={templates}
              selected={editing === it.id}
              onClick={() => it.playlist_id && onOpen(it.id, it.playlist_id)}
            />
          </button>
        ))
      )}
    </div>
  )
}

/**
 * Which order of this song is being looked at: Main, or one of its named ones.
 *
 * Offered only when Main names its groups. An arrangement over a sermon deck of
 * nineteen unnamed slides is a control that does nothing, and one line saying
 * why beats a button that disappoints (ADR 0027).
 */
function ArrangementPicker({
  playlist,
  picked,
  onPick,
}: {
  playlist: PlaylistDetail
  picked: string | null
  onPick: (id: string | null) => void
}) {
  const groups = playlist.groups ?? []
  const list = playlist.arrangements ?? []

  if (!groups.length) {
    return (
      <p className="edit-console__dim">
        Name the groups in Main — a Section on each row — and this song can have named
        arrangements: an order of those groups, for a service that sings it differently.
      </p>
    )
  }

  const add = async () => {
    const made = await post<Arrangement>(keys.arrangements(playlist.id), {
      name: `Arrangement ${list.length + 1}`,
    })
    invalidate(keys.playlist(playlist.id))
    invalidate(keys.arrangements(playlist.id))
    onPick(made.id)
  }

  return (
    <div className="edit-console__arr">
      <FormGroup label="Order">
        <select
          value={picked ?? ''}
          aria-label="Arrangement"
          onChange={(e) => onPick(e.target.value || null)}
        >
          <option value="">Main — every slide, and where they are edited</option>
          {list.map((a) => (
            <option key={a.id} value={a.id}>{a.name}</option>
          ))}
        </select>
      </FormGroup>
      <button type="button" className="edit-console__addbtn" onClick={() => void add()}>
        <Plus size={12} aria-hidden /> New arrangement
      </button>
    </div>
  )
}

/**
 * A named order: which groups, in what order, how many times.
 *
 * **No slide editing here, on purpose.** An arrangement holds group NAMES, not
 * slides — which is what makes it follow Main by construction: add a line to
 * the chorus and every arrangement using it grows one. Editing a slide from
 * inside an arrangement would need it to own that slide, and then the first
 * typo would leave Main and its arrangements singing different words.
 */
function ArrangementEditor({
  playlist,
  arrangement,
  onGone,
}: {
  playlist: PlaylistDetail
  arrangement: Arrangement | null
  onGone: () => void
}) {
  if (!arrangement) return null
  const groups = playlist.groups ?? []
  const seq = arrangement.groups

  const commit = async (next: string[]) => {
    await put(keys.arrangement(arrangement.id), { groups: next })
    invalidate(keys.playlistUnder(playlist.id, arrangement.id))
    invalidate(keys.playlist(playlist.id))
    invalidate(keys.arrangements(playlist.id))
    invalidate(keys.rundowns())
  }

  return (
    <div className="edit-console__arrbody">
      <div className="edit-console__arrhead">
        <input
          className="edit-console__arrname"
          key={arrangement.name}
          defaultValue={arrangement.name}
          aria-label="Arrangement name"
          onKeyDown={(e) => {
            if (e.key === 'Enter') (e.target as HTMLInputElement).blur()
          }}
          onBlur={(e) => {
            const name = e.target.value.trim()
            if (!name || name === arrangement.name) {
              e.target.value = arrangement.name
              return
            }
            void (async () => {
              await put(keys.arrangement(arrangement.id), { name })
              invalidate(keys.playlist(playlist.id))
              invalidate(keys.arrangements(playlist.id))
              invalidate(keys.rundowns())
            })()
          }}
        />
        <button
          type="button"
          className="edit-console__iconbtn"
          aria-label="Delete this arrangement"
          onClick={() => {
            if (!window.confirm(`Delete “${arrangement.name}”? The song and its slides stay.`)) return
            void (async () => {
              await del(keys.arrangement(arrangement.id))
              invalidate(keys.playlist(playlist.id))
              invalidate(keys.arrangements(playlist.id))
              invalidate(keys.rundowns())
              onGone()
            })()
          }}
        >
          <Trash2 size={13} aria-hidden />
        </button>
      </div>

      <div className="edit-console__arrseq" role="list" aria-label="Group order">
        {seq.length === 0 ? (
          <p className="edit-console__dim">Nothing in this order yet. Add a group below.</p>
        ) : (
          seq.map((name, i) => (
            <div key={`${name}:${i}`} role="listitem" className="edit-console__arrgroup">
              <span className="edit-console__arrpos">{String(i + 1).padStart(2, '0')}</span>
              <span className="edit-console__arrlabel">{name}</span>
              <span className="edit-console__arrslides">
                {(() => {
                  const n = groups.find((g) => g.name === name)?.slides ?? 0
                  // Zero means Main no longer has this group — a rename, most
                  // likely. Said plainly rather than shown as "0 slides".
                  return n === 0 ? 'not in Main' : n === 1 ? '1 slide' : `${n} slides`
                })()}
              </span>
              <button
                type="button"
                className="edit-console__iconbtn"
                aria-label={`Move ${name} up`}
                disabled={i === 0}
                onClick={() => void commit(moved(seq, i, -1))}
              >
                <ChevronUp size={13} aria-hidden />
              </button>
              <button
                type="button"
                className="edit-console__iconbtn"
                aria-label={`Move ${name} down`}
                disabled={i === seq.length - 1}
                onClick={() => void commit(moved(seq, i, 1))}
              >
                <ChevronDown size={13} aria-hidden />
              </button>
              <button
                type="button"
                className="edit-console__iconbtn"
                aria-label={`Remove ${name} at ${i + 1}`}
                onClick={() => void commit(seq.filter((_, n) => n !== i))}
              >
                <X size={13} aria-hidden />
              </button>
            </div>
          ))
        )}
      </div>

      <div className="edit-console__pool-btns">
        {groups.map((g) => (
          <button
            key={g.name}
            type="button"
            className="edit-console__addbtn"
            onClick={() => void commit([...seq, g.name])}
          >
            <Plus size={12} aria-hidden /> {g.name}
            <span className="edit-console__addbtn-n">{g.slides}</span>
          </button>
        ))}
      </div>

      <p className="edit-console__hint">
        Slides are edited in <b>Main</b>. This order names groups, so a fix to the chorus reaches
        every arrangement that sings it.
        {playlist.ungrouped?.length
          ? ` ${playlist.ungrouped.length} slide(s) in Main have no group and cannot appear here: ${playlist.ungrouped.join(', ')}.`
          : ''}
      </p>
    </div>
  )
}

/**
 * A playlist's tags, typed as free strings (ADR 0026).
 *
 * On the PLAYLIST rather than the Item, because the rail filters songs:
 * tagging thirty slides "Christmas" would be thirty edits saying one thing.
 */
function TagsEditor({ playlist }: { playlist: PlaylistDetail }) {
  const [adding, setAdding] = useState('')
  const tags = playlist.tags ?? []

  const commit = async (next: string[]) => {
    await put(`${keys.playlist(playlist.id)}/tags`, { tags: next })
    invalidate(keys.playlist(playlist.id))
    invalidate(keys.playlists())
    invalidate(keys.tags())
  }

  return (
    <div className="edit-console__tags">
      {tags.map((t) => (
        <span key={t} className="edit-console__tag">
          {t}
          <button
            type="button"
            aria-label={`Remove tag ${t}`}
            onClick={() => void commit(tags.filter((x) => x !== t))}
          >
            <X size={11} aria-hidden />
          </button>
        </span>
      ))}
      <input
        className="edit-console__tagadd"
        value={adding}
        placeholder="+ tag"
        aria-label="Add a tag"
        onChange={(e) => setAdding(e.target.value)}
        onKeyDown={(e) => {
          if (e.key !== 'Enter') return
          e.preventDefault()
          const t = adding.trim()
          setAdding('')
          if (t && !tags.includes(t)) void commit([...tags, t])
        }}
      />
    </div>
  )
}

/**
 * Copy this Item into another playlist, or move it there.
 *
 * A copy is a NEW Item. Two songs that both say "Verse 1" are two pieces of
 * content, and sharing one across playlists is how a fix to tonight's set
 * silently edits last week's (ADR 0026). Sharing belongs INSIDE a playlist,
 * where it is a repeat.
 */
function CopyMove({
  item,
  from,
  playlists,
  onMoved,
}: {
  item: Item
  from: string
  playlists: PlaylistSummary[]
  onMoved: (playlistId: string) => void
}) {
  const [open, setOpen] = useState<'copy' | 'move' | null>(null)
  const others = playlists.filter((p) => p.id !== from)
  if (!others.length) return null

  const go = async (verb: 'copy' | 'move', to: string) => {
    await post(`${keys.item(item.id)}/${verb}`, { playlist_id: to })
    invalidate(keys.playlist(from))
    invalidate(keys.playlist(to))
    invalidate(keys.playlistItems(from))
    invalidate(keys.playlistItems(to))
    invalidate(keys.playlists())
    invalidate(keys.items())
    invalidate(keys.rundowns())
    setOpen(null)
    if (verb === 'move') onMoved(to)
  }

  return (
    <>
      <button
        type="button"
        className="edit-console__iconbtn"
        aria-label={`Copy ${item.label} to another playlist`}
        onClick={() => setOpen(open === 'copy' ? null : 'copy')}
      >
        <Copy size={13} aria-hidden />
      </button>
      <button
        type="button"
        className="edit-console__iconbtn"
        aria-label={`Move ${item.label} to another playlist`}
        onClick={() => setOpen(open === 'move' ? null : 'move')}
      >
        <CornerUpRight size={13} aria-hidden />
      </button>
      {open ? (
        <select
          className="edit-console__copyto"
          value=""
          autoFocus
          aria-label={open === 'copy' ? `Copy ${item.label} into` : `Move ${item.label} into`}
          onChange={(e) => e.target.value && void go(open, e.target.value)}
          onBlur={() => setOpen(null)}
        >
          <option value="">{open === 'copy' ? 'Copy into…' : 'Move into…'}</option>
          {others.map((p) => <option key={p.id} value={p.id}>{p.name}</option>)}
        </select>
      ) : null}
    </>
  )
}

function PlaylistView({
  id,
  templates,
  playlists,
  editing,
  onEdit,
  onGo,
}: {
  id: string
  templates: TemplateSummary[]
  playlists: PlaylistSummary[]
  editing: string | 'new' | null
  onEdit: (id: string) => void
  onGo: (playlistId: string) => void
}) {
  // Which named order is being looked at. Null is MAIN — the order that owns
  // the slides, and the only one where they can be edited (ADR 0027).
  const [arrangement, setArrangement] = useState<string | null>(null)
  const { data: playlist } = useQuery<PlaylistDetail>(keys.playlistUnder(id, arrangement))
  // The playlist's own Items, arranged or not. A chorus dropped from tonight's
  // arrangement is still the song's chorus (ADR 0026).
  const { data: owned } = useQuery<{ items: Item[] }>(keys.playlistItems(id))
  const contents = playlist?.items ?? []
  const library = owned?.items ?? []
  // How many places each Item occupies, so a repeat is visible as a repeat
  // rather than as a row somebody may think they are seeing twice by mistake.
  const occurrences = new Map<string, number>()
  for (const e of contents) occurrences.set(e.id, (occurrences.get(e.id) ?? 0) + 1)

  const save = useCallback(
    async (entries: { entry?: string; item: string; section?: string | null }[]) => {
      await put(`${keys.playlist(id)}/items`, { items: entries })
      invalidate(keys.playlist(id))
      invalidate(keys.playlists())
      invalidate(keys.arrangements(id))
      invalidate(keys.rundowns())
    },
    [id],
  )

  // An entry keeps its id through a reorder. Minting a new one every time
  // somebody dragged a row would move the operator's cursor with it, which is
  // the kind of thing only ever noticed mid-service.
  const entryOf = (it: Entry) => ({
    entry: it.entry_id,
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
        {/* Editable where it is read. A playlist could be created and never
            renamed, which left a rail full of "Playlist 5". */}
        <input
          className="edit-console__list-title"
          key={playlist.name}
          defaultValue={playlist.name}
          aria-label="Playlist name"
          onKeyDown={(e) => {
            if (e.key === 'Enter') (e.target as HTMLInputElement).blur()
            if (e.key === 'Escape') {
              ;(e.target as HTMLInputElement).value = playlist.name
              ;(e.target as HTMLInputElement).blur()
            }
          }}
          onBlur={(e) => {
            const name = e.target.value.trim()
            if (!name || name === playlist.name) {
              e.target.value = playlist.name
              return
            }
            void (async () => {
              await patch(keys.playlist(id), { name })
              invalidate(keys.playlist(id))
              invalidate(keys.playlists())
              invalidate(keys.rundowns())
            })()
          }}
        />
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

      <TagsEditor playlist={playlist} />

      <ArrangementPicker
        playlist={playlist}
        picked={arrangement}
        onPick={setArrangement}
      />

      {arrangement ? (
        <ArrangementEditor
          playlist={playlist}
          arrangement={playlist.arrangements?.find((a) => a.id === arrangement) ?? null}
          onGone={() => setArrangement(null)}
        />
      ) : null}

      {contents.length === 0 ? (
        <p className="edit-console__dim">
          {library.length
            ? 'Nothing arranged yet. Put its items in order below.'
            : 'Empty. Start with New item — an item belongs to the playlist it is written for.'}
        </p>
      ) : (
        contents.map((it, i) => (
          <div key={it.entry_id} className="edit-console__entry">
            <ItemRow
              item={it}
              index={String(i + 1).padStart(2, '0')}
              repeat={
                (occurrences.get(it.id) ?? 1) > 1
                  ? `${contents.filter((x, n) => x.id === it.id && n <= i).length} of ${occurrences.get(it.id)}`
                  : undefined
              }
              templates={templates}
              selected={editing === it.id}
              onClick={() => onEdit(it.id)}
            />
            <div className="edit-console__entry-side">
              {/* Under a named order these are all read-only: an arrangement
                  holds group NAMES, and the slides belong to Main (ADR 0027).
                  Reordering here would have to mean reordering Main, which is
                  not what the operator asked for. */}
              <input
                className="edit-console__section"
                placeholder="Section"
                disabled={!!arrangement}
                aria-label={`Section for ${it.label}`}
                defaultValue={it.section ?? ''}
                onBlur={(e) => {
                  const section = e.target.value.trim() || null
                  if ((it.section ?? null) === section) return
                  void save(contents.map((x, n) => (n === i ? { item: x.id, section } : entryOf(x))))
                }}
              />
              <div className="edit-console__entry-btns">
                {arrangement ? null : (
                  <CopyMove item={it} from={id} playlists={playlists} onMoved={onGo} />
                )}
                <button
                  type="button"
                  className="edit-console__iconbtn"
                  aria-label={`Move ${it.label} up`}
                  disabled={i === 0 || !!arrangement}
                  onClick={() => void save(moved(contents, i, -1).map(entryOf))}
                >
                  <ChevronUp size={13} aria-hidden />
                </button>
                <button
                  type="button"
                  className="edit-console__iconbtn"
                  aria-label={`Move ${it.label} down`}
                  disabled={i === contents.length - 1 || !!arrangement}
                  onClick={() => void save(moved(contents, i, 1).map(entryOf))}
                >
                  <ChevronDown size={13} aria-hidden />
                </button>
                <button
                  type="button"
                  className="edit-console__iconbtn"
                  disabled={!!arrangement}
                  aria-label={`Remove ${it.label} from the arrangement`}
                  // THIS place, not every place: removing the second chorus
                  // must leave the first one singing. The Item itself survives
                  // either way — it is still the song's chorus.
                  onClick={() => void save(contents.filter((_, n) => n !== i).map(entryOf))}
                >
                  <X size={13} aria-hidden />
                </button>
              </div>
            </div>
          </div>
        ))
      )}

      {library.length && !arrangement ? (
        <div className="edit-console__pool">
          {/* This playlist's OWN items — every one of them, including the ones
              already in the order. Adding one again is how a chorus comes round
              a second time, and it is the same Item, so a typo fixed once is
              fixed everywhere it appears (ADR 0026). */}
          <GroupHeader label="Arrange again" trailing={`${library.length} in this playlist`} />
          <p className="edit-console__dim">
            The same item, in another place. Edit it once and every place it appears follows.
          </p>
          <div className="edit-console__pool-btns">
            {library.map((it) => (
              <button
                key={it.id}
                type="button"
                className="edit-console__addbtn"
                onClick={() => void save([...contents.map(entryOf), { item: it.id }])}
              >
                <Plus size={12} aria-hidden /> {it.label}
                {(occurrences.get(it.id) ?? 0) > 0 ? (
                  <span className="edit-console__addbtn-n">×{occurrences.get(it.id)}</span>
                ) : null}
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
  repeat,
  templates,
  selected,
  onClick,
}: {
  /** "2 of 3" when this Item is arranged more than once in this playlist. */
  repeat?: string
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
        <span className="edit-console__item-title">
          {item.label}
          {/* Said out loud, because two identical rows in a list read as a
              mistake until something says they are the same thing on purpose. */}
          {repeat ? <span className="edit-console__item-repeat">{repeat}</span> : null}
        </span>
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
  screens,
  onChange,
}: {
  value: DestinationDraft[]
  templates: TemplateSummary[]
  outputs: { output_id: string; layers: string[] }[]
  screens: Screen[]
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
            <div className="edit-console__dest-head">
              <span className="edit-console__dest-num">
                {value.length > 1 ? `Destination ${i + 1}` : 'Destination'}
              </span>
              {value.length > 1 ? (
                <button
                  type="button"
                  className="edit-console__iconbtn"
                  aria-label={`Remove destination ${i + 1}`}
                  onClick={() => onChange(value.filter((_, n) => n !== i))}
                >
                  <X size={13} aria-hidden />
                </button>
              ) : null}
            </div>
            <div className="edit-console__dest-row">
              <FormGroup label="Goes to">
                {/* The place, not the plumbing (ADR 0023). Picking a screen
                    means this Item follows it: moving the wall to another node
                    is one edit in Configure, not one per Item. */}
                <select
                  value={d.screen_id}
                  aria-label={`Destination ${i + 1} screen`}
                  onChange={(e) => {
                    const sc = screens.find((s) => s.id === e.target.value)
                    set(i, {
                      screen_id: e.target.value,
                      // A screen with a treatment supplies it; the Item can
                      // still override, which is why this sets rather than
                      // locks.
                      ...(sc?.template_id ? { template_id: sc.template_id } : {}),
                    })
                  }}
                >
                  <option value="">An output on this node</option>
                  {screens.map((s) => <option key={s.id} value={s.id}>{s.name}</option>)}
                </select>
              </FormGroup>
              {d.screen_id ? (
                <FormGroup label="Which is">
                  {/* Readable, not editable: the wiring belongs to the screen.
                      Shown anyway, because an operator debugging a feed needs
                      to see where it actually goes. */}
                  <p className="edit-console__dim">
                    {(() => {
                      const sc = screens.find((s) => s.id === d.screen_id)
                      if (!sc) return 'a screen this server no longer has'
                      const where = sc.node_name ? ` on ${sc.node_name}` : ''
                      return `${sc.output} · ${sc.layer}${where}`
                    })()}
                  </p>
                </FormGroup>
              ) : (
                <>
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
                </>
              )}
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
  /** A named place (ADR 0023). Empty means the wiring below is the answer. */
  screen_id: string
  output: string
  layer: string
  template_id: string
  mode: string
  blank: boolean
}

/** A place in the building, as Configure named it. */
type Screen = {
  id: string
  name: string
  node_id: string | null
  /** The node's own name, filled in here — an operator should never meet an id. */
  node_name?: string | null
  output: string
  layer: string
  template_id: string | null
}

const blankDest = (templates: TemplateSummary[]): DestinationDraft => ({
  screen_id: '',
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
  playlistId,
  onDone,
}: {
  itemId: string | null
  item: Item | null
  /** The playlist a NEW item is written for (ADR 0026). */
  playlistId: string | null
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
        screen_id: d.screen_id ?? '',
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
      screen_id: '',
      output: item?.destination?.output ?? '',
      layer: item?.destination?.layer ?? 'Lower Third',
      template_id: item?.template_id ?? templates[0]?.id ?? '',
      mode: 'auto',
      blank: false,
    }]
  })
  // EVERY destination's fields, not just the first one's.
  //
  // The Item carries one set of data and the server checks it against every
  // non-blank destination (ADR 0018). Showing only the first template's fields
  // meant a wall template asking for `${line}` that the lower third does not
  // declare was invisible — and the Item then could not be saved from the
  // screen that was supposed to author it. Blank destinations render nothing,
  // so they ask for nothing, exactly as the server has it.
  const wanted = [...new Set(dests.filter((d) => !d.blank).map((d) => d.template_id).filter(Boolean))]
  const [manifests, setManifests] = useState<Record<string, Field[]>>({})
  const wantedKey = wanted.join(',')
  useEffect(() => {
    let alive = true
    void Promise.all(
      wanted.map(async (id) => {
        const t = await get<TemplateDetail>(keys.template(id)).catch(() => null)
        return [id, t?.head?.manifest?.fields ?? []] as const
      }),
    ).then((pairs) => {
      if (alive) setManifests(Object.fromEntries(pairs))
    })
    return () => {
      alive = false
    }
    // `wantedKey` and not `wanted`: a fresh array every render would refetch
    // every render.
  }, [wantedKey]) // eslint-disable-line react-hooks/exhaustive-deps

  const nameOf = (id: string) => templates.find((t) => t.id === id)?.name ?? id
  // Union, in destination order, so the first destination's fields come first —
  // that IS the graphic an operator thinks of as "the" one. A field several
  // templates declare is one input; `needed_by` is what makes an unexpected
  // field explicable rather than mysterious.
  const fieldUse = new Map<string, { field: Field; from: string[] }>()
  const clashes: string[] = []
  for (const id of wanted) {
    for (const f of manifests[id] ?? []) {
      const seen = fieldUse.get(f.key)
      if (!seen) {
        fieldUse.set(f.key, { field: { ...f }, from: [nameOf(id)] })
        continue
      }
      seen.from.push(nameOf(id))
      // Required by any is required. Two templates disagreeing about a field's
      // TYPE is not something one value can satisfy, so it is named rather
      // than silently resolved to whichever was read first.
      if (f.required) seen.field.required = true
      if (f.type && seen.field.type && f.type !== seen.field.type) {
        clashes.push(
          `“${f.key}” is a ${seen.field.type} in ${seen.from[0]} and a ${f.type} in ${nameOf(id)}. One Item cannot be both.`,
        )
      }
    }
  }
  const fields = [...fieldUse.values()].map((u) => u.field)
  const multi = wanted.length > 1

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
  const { data: nodes } = useQuery<{ nodes: { node_id: string; name?: string }[] }>(keys.nodes())
  const nodeId = nodes?.nodes?.[0]?.node_id ?? null
  const { data: outs } = useQuery<{ outputs: { output_id: string; layers: string[] }[] }>(
    nodeId ? keys.outputs(nodeId) : null,
  )
  const outputs = outs?.outputs ?? []
  // The places graphics go (ADR 0023). An install with none configured still
  // works: the destination editor falls back to naming an output directly.
  const { data: screenData } = useQuery<{ screens: Screen[] }>(keys.screens())
  // Carrying the node's NAME, because a destination card that says
  // "on C7A9A9ACFB4CACE6F520D7AB0B" has given up on being read.
  const screens = (screenData?.screens ?? []).map((s) => ({
    ...s,
    node_name: nodes?.nodes?.find((n) => n.node_id === s.node_id)?.name ?? s.node_id,
  }))

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
    // Keyed by the field KEYS, not the array: `fields` is rebuilt every render
    // and depending on it re-ran this on every keystroke.
  }, [fields.map((f) => f.key).join(','), item]) // eslint-disable-line react-hooks/exhaustive-deps

  const save = useCallback(async () => {
    setSaving(true)
    setProblems([])
    try {
      const body = {
        // The Item's own template stays the FIRST destination's — it is the
        // graphic an operator thinks of as "the" one, and it is what a
        // pre-ADR-0018 consumer reads.
        template_id: dests[0]?.template_id || item?.template_id || templates[0]?.id || '',
        // An Item belongs to the playlist it was written for. Sent on create
        // only: moving one is its own act, not a side effect of a save.
        ...(itemId ? {} : { playlist_id: playlistId }),
        label: label || undefined,
        data: coerce(fields, data),
        // Resolved to a REVISION here, because a destination pins the revision
        // it was built against — publishing a new one must not silently change
        // what an existing Item renders.
        destinations: dests.map((d) => ({
          // A screen answers where it goes; the server resolves it, and the
          // wiring it resolves to is written beside it as the fallback.
          screen_id: d.screen_id || undefined,
          output: d.screen_id ? undefined : d.output || undefined,
          layer: d.screen_id ? undefined : d.layer || undefined,
          template_rev: templates.find((t) => t.id === d.template_id)?.head_rev ?? undefined,
          mode: d.mode,
          blank: d.blank,
        })),
      }
      if (itemId) await patch(keys.item(itemId), body)
      else await post(keys.items(), body)
      invalidate(keys.items())
      if (playlistId) {
        invalidate(keys.playlist(playlistId))
        invalidate(keys.playlistItems(playlistId))
        invalidate(keys.playlists())
        invalidate(keys.rundowns())
      }
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
  }, [label, data, dests, templates, fields, item, itemId, playlistId, onDone])

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

      {clashes.length > 0 ? (
        <ul className="edit-console__problems" role="alert">
          {clashes.map((c) => (
            <li key={c}>{c}</li>
          ))}
        </ul>
      ) : null}

      {fields.map((f) => {
        const use = fieldUse.get(f.key)
        return (
          <FieldInput
            key={f.key}
            field={f}
            // Only when the destinations DISAGREE about it: annotating every
            // field with every template that wants it is noise on the common
            // case, where they all want the same ones.
            neededBy={
              multi && use && use.from.length < wanted.length ? use.from.join(', ') : undefined
            }
            value={data[f.key] ?? ''}
            onChange={(v) => setData((d) => ({ ...d, [f.key]: v }))}
          />
        )
      })}

      <Destinations
        value={dests}
        templates={templates}
        outputs={outputs}
        screens={screens}
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
  neededBy,
  onChange,
}: {
  field: Field
  value: string
  /** Which templates want this field, when not all of them do (ADR 0018). */
  neededBy?: string
  onChange: (v: string) => void
}) {
  // Count code points, matching the server: `max_len` is about what fits in a
  // box, and an emoji is one character to a human but two UTF-16 units.
  const used = [...value].length
  const over = field.max_len != null && used > field.max_len

  const label = [
    field.label ?? field.key,
    field.required ? '· required' : '',
    // Where an unexpected field came from. Without it, a Line box appearing
    // because the wall template wants one is a mystery.
    neededBy ? `· ${neededBy} only` : '',
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
