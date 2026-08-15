/**
 * Building the running order: Playlists, and the Rundown that orders them.
 *
 * # Reordering is a whole-list write
 *
 * Move an entry and the entire list is sent back. Per-position edits from two
 * operators dragging at once produce an order neither of them chose; replacing
 * the list wholesale means the second write wins cleanly and *visibly*, which
 * is honest until there is a reason to build real collaborative editing.
 *
 * # No drag-and-drop
 *
 * Up and down buttons, on purpose. jsdom has no layout, so a simulated drag
 * certifies nothing — a mode whose only path to reordering is a drag is a mode
 * whose reordering is untested. Drag can be added later *alongside* these.
 */

import { useCallback, useState } from 'react'
import { ChevronDown, ChevronUp, Plus, X } from 'lucide-react'
import { useQuery, invalidate } from '../../lib/useQuery'
import { keys } from '../../lib/keys'
import { post, put } from '../../lib/api'
import type { Item } from '../show/types'

type Playlist = { id: string; name: string; items: Item[] }
type PlaylistSummary = { id: string; name: string; items: number }
type Rundown = { id: string; name: string; playlists: Playlist[] }

/** Move `id` by `delta` and return the new order. */
function moved<T>(list: T[], index: number, delta: number): T[] {
  const to = index + delta
  if (to < 0 || to >= list.length) return list
  const copy = [...list]
  const [entry] = copy.splice(index, 1)
  copy.splice(to, 0, entry as T)
  return copy
}

export function Running({ items }: { items: Item[] }) {
  const { data: playlists } = useQuery<{ playlists: PlaylistSummary[] }>(keys.playlists())
  const { data: rundowns } = useQuery<{ rundowns: { id: string; name: string }[] }>(keys.rundowns())
  const [selected, setSelected] = useState<string | null>(null)

  const list = playlists?.playlists ?? []
  const rundownId = rundowns?.rundowns?.[0]?.id ?? null

  const addPlaylist = useCallback(async () => {
    const p = await post<PlaylistSummary>(keys.playlists(), { name: `Playlist ${list.length + 1}` })
    invalidate(keys.playlists())
    setSelected(p.id)
  }, [list.length])

  const addRundown = useCallback(async () => {
    await post(keys.rundowns(), { name: 'Sunday' })
    invalidate(keys.rundowns())
  }, [])

  return (
    <div className="running">
      <section>
        <div className="edit__list-head">
          <h2>Playlists</h2>
          <button type="button" className="btn btn--ghost btn--small" onClick={() => void addPlaylist()}>
            <Plus size={14} aria-hidden /> New
          </button>
        </div>
        {list.length === 0 ? (
          <p className="show__dim show__pad">None yet.</p>
        ) : (
          <ul className="edit__items">
            {list.map((p) => (
              <li key={p.id} className={selected === p.id ? 'edit__item edit__item--selected' : 'edit__item'}>
                <button type="button" className="edit__item-btn" onClick={() => setSelected(p.id)}>
                  <span className="edit__item-label">{p.name}</span>
                  <span className="show__dim">{p.items} item(s)</span>
                </button>
              </li>
            ))}
          </ul>
        )}
        {selected ? <PlaylistEditor id={selected} allItems={items} /> : null}
      </section>

      <section>
        <div className="edit__list-head">
          <h2>Rundown</h2>
          {!rundownId ? (
            <button type="button" className="btn btn--ghost btn--small" onClick={() => void addRundown()}>
              <Plus size={14} aria-hidden /> New
            </button>
          ) : null}
        </div>
        {rundownId ? <RundownEditor id={rundownId} playlists={list} /> : (
          <p className="show__dim show__pad">No rundown yet.</p>
        )}
      </section>
    </div>
  )
}

function PlaylistEditor({ id, allItems }: { id: string; allItems: Item[] }) {
  const { data: playlist } = useQuery<Playlist>(keys.playlist(id))
  const contents = playlist?.items ?? []
  const inList = new Set(contents.map((i) => i.id))

  const save = useCallback(
    async (ids: string[]) => {
      await put(`${keys.playlist(id)}/items`, { items: ids })
      invalidate(keys.playlist(id))
      invalidate(keys.playlists())
      invalidate(keys.rundowns())
    },
    [id],
  )

  return (
    <div className="running__editor">
      <h3>{playlist?.name}</h3>
      {contents.length === 0 ? (
        <p className="show__dim">Empty. Add items below.</p>
      ) : (
        <ol className="running__order">
          {contents.map((it, i) => (
            <li key={it.id}>
              <span className="running__pos">{i + 1}</span>
              <span className="running__label">{it.label}</span>
              <button type="button" className="btn btn--ghost btn--small" aria-label={`Move ${it.label} up`}
                      disabled={i === 0}
                      onClick={() => void save(moved(contents, i, -1).map((x) => x.id))}>
                <ChevronUp size={13} aria-hidden />
              </button>
              <button type="button" className="btn btn--ghost btn--small" aria-label={`Move ${it.label} down`}
                      disabled={i === contents.length - 1}
                      onClick={() => void save(moved(contents, i, 1).map((x) => x.id))}>
                <ChevronDown size={13} aria-hidden />
              </button>
              <button type="button" className="btn btn--ghost btn--small" aria-label={`Remove ${it.label}`}
                      onClick={() => void save(contents.filter((x) => x.id !== it.id).map((x) => x.id))}>
                <X size={13} aria-hidden />
              </button>
            </li>
          ))}
        </ol>
      )}

      <div className="running__add">
        {allItems.filter((it) => !inList.has(it.id)).map((it) => (
          <button key={it.id} type="button" className="btn btn--ghost btn--small"
                  onClick={() => void save([...contents.map((x) => x.id), it.id])}>
            <Plus size={12} aria-hidden /> {it.label}
          </button>
        ))}
      </div>
    </div>
  )
}

function RundownEditor({ id, playlists }: { id: string; playlists: PlaylistSummary[] }) {
  const { data: rundown } = useQuery<Rundown>(keys.rundown(id))
  const contents = rundown?.playlists ?? []
  const inRundown = new Set(contents.map((p) => p.id))

  const save = useCallback(
    async (ids: string[]) => {
      await put(`${keys.rundown(id)}/playlists`, { playlists: ids })
      invalidate(keys.rundown(id))
      invalidate(keys.rundowns())
    },
    [id],
  )

  return (
    <div className="running__editor">
      <h3>{rundown?.name}</h3>
      {contents.length === 0 ? (
        <p className="show__dim">Empty. Add playlists below.</p>
      ) : (
        <ol className="running__order">
          {contents.map((p, i) => (
            <li key={p.id}>
              <span className="running__pos">{i + 1}</span>
              <span className="running__label">
                {p.name} <span className="show__dim">{p.items.length} item(s)</span>
              </span>
              <button type="button" className="btn btn--ghost btn--small" aria-label={`Move ${p.name} up`}
                      disabled={i === 0}
                      onClick={() => void save(moved(contents, i, -1).map((x) => x.id))}>
                <ChevronUp size={13} aria-hidden />
              </button>
              <button type="button" className="btn btn--ghost btn--small" aria-label={`Move ${p.name} down`}
                      disabled={i === contents.length - 1}
                      onClick={() => void save(moved(contents, i, 1).map((x) => x.id))}>
                <ChevronDown size={13} aria-hidden />
              </button>
              <button type="button" className="btn btn--ghost btn--small" aria-label={`Remove ${p.name}`}
                      onClick={() => void save(contents.filter((x) => x.id !== p.id).map((x) => x.id))}>
                <X size={13} aria-hidden />
              </button>
            </li>
          ))}
        </ol>
      )}

      <div className="running__add">
        {playlists.filter((p) => !inRundown.has(p.id)).map((p) => (
          <button key={p.id} type="button" className="btn btn--ghost btn--small"
                  onClick={() => void save([...contents.map((x) => x.id), p.id])}>
            <Plus size={12} aria-hidden /> {p.name}
          </button>
        ))}
      </div>
    </div>
  )
}
