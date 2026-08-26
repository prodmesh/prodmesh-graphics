/**
 * Configure — for whoever set up the room.
 *
 * Node discovery and adoption, outputs, layer stacks, colour, and test
 * patterns, in the console language (ADR 0020).
 *
 * # Adoption is a real boundary, not a formality
 *
 * A node that dials this server is welcomed and then given **nothing** until
 * someone adopts it. On a shared church network the thing that must not quietly
 * start receiving this week's graphics is a laptop somebody left running a
 * stale build — so an unadopted node appears here as a stranger asking to join,
 * and cues to it are refused with a sentence pointing back at this mode.
 *
 * # Discovery is an aid, never an authority
 *
 * The server advertises over mDNS so a node can find it with nothing typed in.
 * Multicast is unreliable across VLANs and behind switches with poor IGMP
 * snooping, so a node can always be pointed at an address by hand — and this
 * mode says so rather than presenting discovery as the only way in.
 */

import { useCallback, useState } from 'react'
import { Check, ChevronDown, ChevronUp, Plus, Trash2, X } from 'lucide-react'
import { useQuery, invalidate } from '../../lib/useQuery'
import { keys } from '../../lib/keys'
import { ApiError, del, patch, post, put } from '../../lib/api'
import { identityChanged, requestSignIn, useIdentity } from '../../lib/identity'
import {
  Button,
  FormGroup,
  GroupHeader,
  KeyValueRow,
  NodeCard,
  StatusPill,
  Tag,
} from '../../components/console'
import type { NodeSummary } from '../show/types'

type ColorConfig = {
  matrix: string
  range: string
  key_range: string
  key_mode: string
}

/** What each pattern proves (`docs/specs/color-and-key.md` §7). */
const PATTERNS: { id: string; label: string; proves: string }[] = [
  { id: 'ramp', label: 'Marked ramp', proves: 'Range marks: 16/235 narrow, 0/255 full' },
  { id: 'bars', label: '75% colour bars', proves: 'Matrix and primary alignment' },
  { id: 'softedge', label: 'Soft-edged white', proves: 'Premultiplication over light and dark video' },
  { id: 'alpha', label: 'Alpha staircase', proves: 'key range and linearity' },
  { id: 'line', label: 'Moving 1px line', proves: 'Frame timing and late-frame visibility' },
]

type Capabilities = {
  /** One connector, as the node's own probe found it — never as anyone typed it. */
  /** Sound devices this node can send audio to instead of embedding it. */
  audio_devices?: {
    host: string
    name: string
    channels: number
    supports_rate: boolean
    default: boolean
  }[]
  outputs?: {
    device: string
    kind: string
    index?: number
    /** The display modes this connector reports. Empty before a node connects. */
    modes?: string[]
    supports_key_fill: boolean
    has_genlock: boolean
    genlock_locked?: boolean
    ref_format?: string | null
    audio_pairs?: number
  }[]
  gpu?: { adapter: string; backend: string } | null
  max_layers?: number
  scene_reader_major?: number
  decoders?: string[]
}

type FullNode = NodeSummary & { adopted: boolean; capabilities: Capabilities | null }

type User = { id: string; username: string; display_name: string | null; created_at: number }
type Group = { id: string; name: string; permissions: string | string[] }
type Station = { id: string; name: string; trusted: boolean }

/** A named place graphics go (ADR 0023). */
type Screen = {
  id: string
  name: string
  node_id: string | null
  output: string
  layer: string
  template_id: string | null
}

/**
 * Configure's sections.
 *
 * A rail rather than another row of tabs: every other mode already has one —
 * Playlists in Edit, the rundown in Show — so this is the pattern the console
 * already has rather than a second kind of navigation. It also has somewhere
 * to grow: audio routing and preferences are coming, and a single scrolling
 * page was already long with three sections on it.
 */
const SECTIONS = [
  { id: 'nodes', label: 'Playout nodes' },
  { id: 'screens', label: 'Screens' },
  { id: 'users', label: 'Users & stations' },
] as const
type SectionId = (typeof SECTIONS)[number]['id']

export function Configure() {
  const { data } = useQuery<{ nodes: FullNode[] }>(keys.nodes())
  const nodes = data?.nodes ?? []
  const adopted = nodes.filter((n) => n.adopted)
  const strangers = nodes.filter((n) => !n.adopted)
  const [section, setSection] = useState<SectionId>('nodes')

  return (
    <div className="cfg-console">
      <div className="console__toolbar">
        <div className="console__toolbar-group">
          <Tag pill tone="raised">{SECTIONS.find((s) => s.id === section)?.label}</Tag>
          <span className="console__toolbar-note">
            Nodes discover this server automatically. Manual address: --server &lt;host:port&gt;.
          </span>
        </div>
        <Tag size={10}>{`${adopted.length} adopted`}</Tag>
      </div>

      <div className="cfg-console__panels">
        <nav className="cfg-console__rail" aria-label="Configure sections">
          {SECTIONS.map((s) => (
            <button
              key={s.id}
              type="button"
              aria-current={section === s.id ? 'page' : undefined}
              className={section === s.id ? 'cfg-console__railitem cfg-console__railitem--on' : 'cfg-console__railitem'}
              onClick={() => setSection(s.id)}
            >
              {s.label}
              {s.id === 'nodes' && strangers.length ? (
                // A node waiting to be adopted is the one thing here that needs
                // answering rather than browsing.
                <Tag size={10} tone="armed">{`${strangers.length} new`}</Tag>
              ) : null}
            </button>
          ))}
        </nav>

        <div className="mode-scroll cfg-console__scroll">
          {section === 'nodes' ? (
            <>
              {strangers.length > 0 ? (
                <section className="cfg-console__section" aria-label="New nodes">
                  <GroupHeader label="New nodes" trailing={`${strangers.length}`} />
                  {strangers.map((n) => (
                    <Stranger key={n.node_id} node={n} />
                  ))}
                </section>
              ) : null}

              <section className="cfg-console__section" aria-label="Playout nodes">
                <GroupHeader label="Playout nodes" />
                {adopted.length === 0 ? (
                  <div className="mode__stub">
                    <p>No playout node has been adopted yet.</p>
                    <p>
                      Start a node and point it at this server. For a manual connection, use{' '}
                      <code>--server &lt;host:port&gt;</code>.
                    </p>
                  </div>
                ) : (
                  adopted.map((n) => <AdoptedNode key={n.node_id} node={n} />)
                )}
              </section>
            </>
          ) : section === 'screens' ? (
            <Screens nodes={adopted} />
          ) : (
            <Users />
          )}
        </div>
      </div>
    </div>
  )
}

/** The modes an operator can pick, and the raster each one implies. */
const MODES: { name: string; width: number; height: number; fps_num: number; fps_den: number }[] = [
  { name: '1080p59.94', width: 1920, height: 1080, fps_num: 60000, fps_den: 1001 },
  { name: '1080p50', width: 1920, height: 1080, fps_num: 50, fps_den: 1 },
  { name: '1080p30', width: 1920, height: 1080, fps_num: 30, fps_den: 1 },
  { name: '1080p25', width: 1920, height: 1080, fps_num: 25, fps_den: 1 },
  { name: '1080i59.94', width: 1920, height: 1080, fps_num: 30000, fps_den: 1001 },
  { name: '720p59.94', width: 1280, height: 720, fps_num: 60000, fps_den: 1001 },
  { name: '2160p29.97', width: 3840, height: 2160, fps_num: 30000, fps_den: 1001 },
]

type OutputConfig = {
  id: string
  backend: string
  /** Where this output's audio goes. Empty is embedded in the video (ADR 0028). */
  audio_device?: string
  audio_host?: string
  ndi_name?: string
  device?: number
  key_device?: number | null
  mode?: string
  width?: number
  height?: number
  fps_num?: number
  fps_den?: number
  audio_channels?: number
  preview_port?: number
  group?: string
}

type OutputsAck = {
  gen: number
  applied: OutputConfig[]
  problems?: string[]
  refused_on_air?: string[]
}

/**
 * A node's output HARDWARE (ADR 0024).
 *
 * Until this existed, changing NDI to DeckLink meant editing a shell command
 * and restarting the process. The devices come from the node's OWN capability
 * report, so this offers what the machine actually has rather than what someone
 * typed.
 *
 * Saving closes a device and opens it again — for the outputs that changed, and
 * only those. An output with a graphic on it is refused unless the operator
 * says to interrupt it.
 */
function OutputHardware({ node }: { node: FullNode }) {
  const { data } = useQuery<{ outputs: OutputConfig[]; ack: OutputsAck | null }>(
    keys.outputConfig(node.node_id),
  )
  // What the NODE says it is running, which is the truth if the two differ.
  const live: OutputConfig[] = (node.state?.outputs ?? []).map((o) => ({
    id: o.id,
    backend: 'ndi',
  }))
  const [draft, setDraft] = useState<OutputConfig[] | null>(null)
  const [problem, setProblem] = useState<string | null>(null)
  const [note, setNote] = useState<string | null>(null)
  const rows = draft ?? (data?.outputs?.length ? data.outputs : live)
  const devices = node.capabilities?.outputs ?? []
  const soundDevices = node.capabilities?.audio_devices ?? []

  const set = (i: number, changes: Partial<OutputConfig>) =>
    setDraft(rows.map((o, n) => (n === i ? { ...o, ...changes } : o)))

  const save = async (force: boolean) => {
    setProblem(null)
    setNote(null)
    try {
      const r = await put<{ delivered: boolean }>(keys.outputConfig(node.node_id), {
        outputs: rows,
        force,
      })
      setDraft(null)
      invalidate(keys.outputConfig(node.node_id))
      invalidate(keys.nodes())
      setNote(
        r.delivered
          ? 'Sent. The node applies it and answers with what it actually did.'
          : 'Saved. This node is offline, so it will be applied when it connects.',
      )
    } catch (e) {
      setProblem(e instanceof Error ? e.message : String(e))
    }
  }

  const ack = data?.ack
  return (
    <details className="cfg-console__diag">
      <summary>Output hardware</summary>

      {rows.map((o, i) => (
        <div key={`${o.id}:${i}`} className="cfg-console__screen-row">
          <FormGroup label="Id">
            <input
              value={o.id}
              aria-label={`Output ${i + 1} id`}
              onChange={(e) => set(i, { id: e.target.value })}
            />
          </FormGroup>
          <FormGroup label="Sends by">
            <select
              value={o.backend}
              aria-label={`Output ${o.id} backend`}
              onChange={(e) => set(i, { backend: e.target.value })}
            >
              <option value="ndi">NDI</option>
              <option value="decklink">SDI (DeckLink)</option>
              <option value="window">A window on this machine</option>
              <option value="headless">Headless (render only)</option>
            </select>
          </FormGroup>

          {o.backend === 'ndi' ? (
            <FormGroup label="Seen on the network as">
              <input
                value={o.ndi_name ?? ''}
                aria-label={`Output ${o.id} NDI name`}
                onChange={(e) => set(i, { ndi_name: e.target.value })}
              />
            </FormGroup>
          ) : null}

          {o.backend === 'decklink' ? (
            <>
              <FormGroup label="Fill">
                {/* From the node's own capability report — what the machine
                    actually has, not what somebody typed. */}
                <select
                  value={String(o.device ?? 0)}
                  aria-label={`Output ${o.id} device`}
                  onChange={(e) => set(i, { device: Number(e.target.value) })}
                >
                  {devices.length === 0 ? <option value="0">no card reported</option> : null}
                  {devices.map((d, n) => (
                    <option key={d.device} value={n}>
                      {d.device}
                      {d.supports_key_fill ? ' · key+fill' : ''}
                      {d.has_genlock ? ' · genlock' : ''}
                    </option>
                  ))}
                </select>
              </FormGroup>
              <FormGroup label="Key">
                <select
                  value={o.key_device == null ? '' : String(o.key_device)}
                  aria-label={`Output ${o.id} key device`}
                  onChange={(e) =>
                    set(i, { key_device: e.target.value === '' ? null : Number(e.target.value) })
                  }
                >
                  <option value="">Fill only</option>
                  {devices.map((d, n) => (
                    <option key={d.device} value={n}>{d.device}</option>
                  ))}
                </select>
              </FormGroup>
              <FormGroup label="Mode">
                {/* Only what THIS connector says it can do. The raster still
                    travels with the name — the node keeps no mode table, and
                    two tables that must agree is one more than there should be
                    — but offering a mode the card will refuse at open time is a
                    choice that fails silently until the output is armed. */}
                <select
                  value={o.mode ?? ''}
                  aria-label={`Output ${o.id} mode`}
                  onChange={(e) => {
                    const m = MODES.find((x) => x.name === e.target.value)
                    set(i, m ? { mode: m.name, width: m.width, height: m.height, fps_num: m.fps_num, fps_den: m.fps_den } : { mode: '' })
                  }}
                >
                  <option value="">Choose a mode</option>
                  {(() => {
                    const card = devices[o.device ?? 0]
                    // Before a node has reported, every mode we know how to
                    // describe. A picker that empties itself because nothing
                    // has connected yet reads as broken.
                    const reported = card?.modes ?? []
                    const offered = reported.length
                      ? MODES.filter((m) => reported.includes(m.name))
                      : MODES
                    return offered.map((m) => <option key={m.name} value={m.name}>{m.name}</option>)
                  })()}
                  {/* A mode already saved that this connector does not offer —
                      the card was swapped, or the config came from elsewhere.
                      Kept visible and named, because silently dropping it would
                      make the field look empty rather than wrong. */}
                  {o.mode && !MODES.some((m) => m.name === o.mode) ? (
                    <option value={o.mode}>{o.mode} (unavailable)</option>
                  ) : null}
                </select>
              </FormGroup>
            </>
          ) : null}

          <FormGroup label="Audio channels">
            <select
              value={String(o.audio_channels ?? 0)}
              aria-label={`Output ${o.id} audio`}
              onChange={(e) => set(i, { audio_channels: Number(e.target.value) })}
            >
              <option value="0">None</option>
              <option value="2">2</option>
              <option value="8">8</option>
              <option value="16">16</option>
            </select>
          </FormGroup>

          {/* Where those channels go (ADR 0028). Offered only when there is
              audio to send — a destination for nothing is a question with no
              consequence. */}
          {(o.audio_channels ?? 0) > 0 ? (
            <FormGroup label="Audio goes to">
              <select
                value={o.audio_device ?? ''}
                aria-label={`Output ${o.id} audio destination`}
                onChange={(e) => {
                  const picked = soundDevices.find((d) => d.name === e.target.value)
                  set(i, { audio_device: e.target.value, audio_host: picked?.host ?? '' })
                }}
              >
                <option value="">
                  {o.backend === 'decklink' ? 'Embedded in the SDI' : 'Embedded in the video'}
                </option>
                {soundDevices.map((d) => (
                  <option key={`${d.host}:${d.name}`} value={d.name}>
                    {d.name} · {d.channels} ch
                    {d.supports_rate ? '' : ' · not 48 kHz'}
                  </option>
                ))}
              </select>
            </FormGroup>
          ) : null}
          {o.audio_device ? (
            <p className="cfg-console__note">
              This output’s audio leaves on <b>{o.audio_device}</b> and is <b>not</b> embedded in
              the video. The sound device has its own clock, so the node holds the two together and
              reports how hard it is working.
            </p>
          ) : null}

          <FormGroup label="Clock group">
            {/* Outputs naming the same group share a thread, which is what
                phase-locked together means in practice (ADR 0011) — and what
                decides which outputs a change interrupts. */}
            <input
              value={o.group ?? 'default'}
              aria-label={`Output ${o.id} group`}
              onChange={(e) => set(i, { group: e.target.value })}
            />
          </FormGroup>
          <button
            type="button"
            className="edit-console__iconbtn"
            aria-label={`Remove output ${o.id}`}
            onClick={() => setDraft(rows.filter((_, n) => n !== i))}
          >
            <Trash2 size={13} aria-hidden />
          </button>
        </div>
      ))}

      <div className="cfg-console__screen-add">
        <button
          type="button"
          className="edit-console__addbtn"
          onClick={() =>
            setDraft([
              ...rows,
              { id: `out-${rows.length}`, backend: 'ndi', ndi_name: `${node.name} ${rows.length}`, group: `g${rows.length}` },
            ])
          }
        >
          <Plus size={12} aria-hidden /> Add an output
        </button>
        <Button variant="secondary" disabled={!draft} onClick={() => void save(false)}>
          Apply
        </Button>
        {ack?.refused_on_air?.length ? (
          // Only offered once the node has said what it would not do — an
          // "interrupt anyway" button on a quiet plant is a trap with no
          // purpose.
          <Button variant="secondary" onClick={() => void save(true)}>
            Apply anyway (interrupts {ack.refused_on_air.join(', ')})
          </Button>
        ) : null}
        {draft ? (
          <Button variant="quiet" onClick={() => setDraft(null)}>Discard</Button>
        ) : null}
      </div>

      {problem ? <p className="tl__problem" role="alert">{problem}</p> : null}
      {note ? <p className="cfg-console__note" role="status">{note}</p> : null}
      {ack?.problems?.length ? (
        <p className="tl__problem" role="alert">
          The node could not apply all of it: {ack.problems.join('; ')}
        </p>
      ) : null}
      {ack?.refused_on_air?.length ? (
        <p className="cfg-console__note">
          Left unchanged because {ack.refused_on_air.join(', ')} is on air. Closing the device
          would remove the graphic.
        </p>
      ) : null}
    </details>
  )
}

/**
 * Users, groups, and stations (ADR 0013).
 *
 * The footer has been telling operators to "add a user in Configure" since M2,
 * and there was no such page — a sentence pointing at a door that did not
 * exist. The server side has been complete the whole time.
 *
 * A server with no users is OPEN, deliberately: a booth that cannot put a
 * graphic up because nobody can remember a PIN is worse than one anybody on the
 * LAN can drive. Adding the first user is what turns permissions on, and this
 * page says so rather than letting it be a surprise.
 */
function Users() {
  const { data, error } = useQuery<{ users: User[]; groups: Group[] }>(keys.users())
  const { data: stationData } = useQuery<{ stations: Station[] }>(keys.stations())
  const users = data?.users ?? []
  const groups = data?.groups ?? []
  const stations = stationData?.stations ?? []
  const me = useIdentity()

  const [username, setUsername] = useState('')
  const [pin, setPin] = useState('')
  const [group, setGroup] = useState('')
  const [problem, setProblem] = useState<string | null>(null)
  const [note, setNote] = useState<string | null>(null)

  const run = async (fn: () => Promise<unknown>) => {
    setProblem(null)
    try {
      await fn()
      invalidate(keys.users())
      invalidate(keys.stations())
    } catch (e) {
      setProblem(e instanceof Error ? e.message : String(e))
    }
  }

  // A 403 here is not a broken page: it is this server telling you that
  // somebody else already turned permissions on and you are not one of them.
  if (error) {
    return (
      <section className="cfg-console__section" aria-label="Users">
        <GroupHeader label="Users & stations" />
        <div className="mode__stub">
          {/* Survives the switch into this branch on purpose: adding the first
              user is what CAUSED the refusal, and "you are now an
              administrator" is the one sentence that makes the locked page
              make sense. Losing it here would leave the operator staring at a
              door with no idea they hold the key. */}
          {note ? <p role="status">{note}</p> : null}
          {me?.no_administrator ? (
            // Signing in cannot fix this one, and offering it as the answer
            // sends somebody round a loop that has no exit: no account holds
            // the permission, so every attempt is refused the same way.
            <>
              <p>
                <strong>No account on this server can manage users.</strong> Signing in as anybody
                will be refused the same way.
              </p>
              <p>
                Run <code>npm run grant-admin -- &lt;username&gt;</code> on the machine running the
                server.
              </p>
            </>
          ) : (
            <>
              <p>This server will not show you its users.</p>
              <p>Sign in as someone who holds <code>graphics.users.manage</code>.</p>
              {/* Naming the missing authority and then leaving the operator to
                  find the door is half an answer. This is the door. */}
              <Button
                variant="secondary"
                onClick={() =>
                  requestSignIn({
                    permission: 'graphics.users.manage',
                    label: 'Manage users and groups',
                  })
                }
              >
                Sign in
              </Button>
            </>
          )}
        </div>
      </section>
    )
  }

  return (
    <>
      <section className="cfg-console__section" aria-label="Users">
        <GroupHeader label="Users" trailing={`${users.length}`} />
        <p className="cfg-console__note">
          {users.length === 0
            ? 'Open access is enabled. Add the first user to turn on permissions; that user becomes an Administrator.'
            : 'Permissions are on. A station with nobody signed in can do only what it is trusted to do.'}
        </p>

        {users.map((u) => (
          <div key={u.id} className="cfg-console__userrow">
            <span className="cfg-console__username">{u.display_name || u.username}</span>
            <span className="cfg-console__userid">{u.username}</span>
          </div>
        ))}

        <div className="cfg-console__screen-row">
          <FormGroup label="Username">
            <input value={username} aria-label="New username" onChange={(e) => setUsername(e.target.value)} />
          </FormGroup>
          <FormGroup label="PIN (4+ digits)">
            <input
              type="password"
              value={pin}
              aria-label="New user PIN"
              onChange={(e) => setPin(e.target.value)}
            />
          </FormGroup>
          <FormGroup label="Group">
            <select value={group} aria-label="New user group" onChange={(e) => setGroup(e.target.value)}>
              <option value="">No group (sign-in only)</option>
              {groups.map((g) => <option key={g.id} value={g.id}>{g.name}</option>)}
            </select>
          </FormGroup>
          <Button
            variant="secondary"
            disabled={!username.trim() || pin.length < 4}
            onClick={() =>
              void run(async () => {
                const made = await post<{ granted_admin?: boolean }>(keys.users(), {
                  username: username.trim(),
                  pin,
                  groups: group ? [group] : [],
                })
                // The first user is made an administrator so that securing the
                // server does not lock it. Said out loud, because a permission
                // nobody knows was granted is a permission nobody audits.
                setNote(
                  made?.granted_admin
                    ? `${username.trim()} is now an Administrator. Permissions are on.`
                    : null,
                )
                setUsername('')
                setPin('')
                // The first user turns permissions ON. The shell is holding an
                // answer from before that happened — an open server, no way to
                // sign in — and it is the footer of this very page.
                identityChanged()
              })
            }
          >
            Add user
          </Button>
        </div>
        {problem ? <p className="tl__problem" role="alert">{problem}</p> : null}
        {note ? <p className="cfg-console__note" role="status">{note}</p> : null}
      </section>

      <section className="cfg-console__section" aria-label="Groups">
        <GroupHeader label="Groups" trailing={`${groups.length}`} />
        <p className="cfg-console__note">
          What a group may do. <code>*</code> is everything, and is what an administrator holds.
        </p>
        {groups.map((g) => (
          <div key={g.id} className="cfg-console__userrow">
            <span className="cfg-console__username">{g.name}</span>
            <span className="cfg-console__userid">
              {(() => {
                const perms: string[] = Array.isArray(g.permissions)
                  ? g.permissions
                  : JSON.parse(String(g.permissions ?? '[]'))
                return perms.includes('*') ? 'everything' : perms.join(' · ') || 'nothing yet'
              })()}
            </span>
          </div>
        ))}
      </section>

      <section className="cfg-console__section" aria-label="Stations">
        <GroupHeader label="Stations" trailing={`${stations.length}`} />
        <p className="cfg-console__note">
          Trusted stations can operate without a signed-in user. Review them regularly.
        </p>
        {stations.map((st) => (
          <div key={st.id} className="cfg-console__userrow">
            {/* A browser names itself when it registers, so that an unattended
                display exists to be trusted before anybody signs in on it.
                That name is a placeholder for the PLACE, and this is where a
                person replaces it. */}
            <input
              className="cfg-console__stationname"
              defaultValue={st.name}
              aria-label={`Station ${st.name} name`}
              onBlur={(e) => {
                const name = e.target.value.trim()
                if (name && name !== st.name) void run(() => put(`/api/stations/${st.id}`, { name }))
              }}
            />
            {st.id === me?.station?.id ? (
              <span className="cfg-console__userid">this browser</span>
            ) : null}
            <label className="cfg-console__trust">
              <input
                type="checkbox"
                checked={st.trusted}
                aria-label={`Station ${st.name} trusted`}
                onChange={(e) =>
                  void run(() => put(`/api/stations/${st.id}/trust`, { trusted: e.target.checked }))
                }
              />
              Trusted
            </label>
          </div>
        ))}
        {stations.length === 0 ? (
          <p className="cfg-console__note">No station has registered yet.</p>
        ) : null}
      </section>
    </>
  )
}

/**
 * Screens — the named places graphics go (ADR 0023).
 *
 * The first thing in Configure that is about the ROOM rather than about a
 * machine. Naming a place here means an Item can say "LED Wall" instead of
 * `booth-1 / wall / Graphics`, and moving that wall later is one edit here
 * rather than one per Item.
 */
function Screens({ nodes }: { nodes: FullNode[] }) {
  const { data } = useQuery<{ screens: Screen[] }>(keys.screens())
  const screens = data?.screens ?? []
  const { data: templateData } = useQuery<{ templates: { id: string; name: string }[] }>(
    keys.templates(),
  )
  const templates = templateData?.templates ?? []
  const [name, setName] = useState('')
  const [problem, setProblem] = useState<string | null>(null)

  // Every output any adopted node REPORTS — the node's own account of itself,
  // never our idea of it — so a screen is built from what the plant actually
  // has rather than from something typed.
  const outputs = nodes.flatMap((n) =>
    (n.state?.outputs ?? []).map((o) => ({
      node_id: n.node_id,
      // The node's NAME for the label — "booth-1", not a 26-character id. The
      // whole point of a screen is that it reads as a place.
      node_name: n.name || n.node_id,
      output_id: o.id,
      layers: o.layers.map((l) => l.name),
    })),
  )

  const run = async (fn: () => Promise<unknown>) => {
    setProblem(null)
    try {
      await fn()
      invalidate(keys.screens())
    } catch (e) {
      setProblem(e instanceof Error ? e.message : String(e))
    }
  }

  return (
    <section className="cfg-console__section" aria-label="Screens">
      <GroupHeader label="Screens" trailing={`${screens.length}`} />
      <p className="cfg-console__note">
        A place in the building, named once. Items go to a screen rather than to an output, so
        moving one is a single edit here.
      </p>

      {screens.map((s) => (
        <div key={s.id} className="cfg-console__screen">
          <div className="cfg-console__screen-row">
            <FormGroup label="Name">
              <input
                key={`n:${s.id}:${s.name}`}
                defaultValue={s.name}
                aria-label={`Screen ${s.name} name`}
                onBlur={(e) => {
                  if (e.target.value.trim() && e.target.value !== s.name) {
                    void run(() => patch(`/api/screens/${s.id}`, { name: e.target.value.trim() }))
                  }
                }}
              />
            </FormGroup>
            <FormGroup label="Output">
              <select
                value={`${s.node_id ?? ''}|${s.output}`}
                aria-label={`Screen ${s.name} output`}
                onChange={(e) => {
                  const [node_id, output] = e.target.value.split('|')
                  void run(() => patch(`/api/screens/${s.id}`, { node_id: node_id || null, output }))
                }}
              >
                {outputs.map((o) => (
                  <option key={`${o.node_id}|${o.output_id}`} value={`${o.node_id}|${o.output_id}`}>
                    {o.output_id} · {o.node_name}
                  </option>
                ))}
                {!outputs.some((o) => o.output_id === s.output) ? (
                  <option value={`${s.node_id ?? ''}|${s.output}`}>
                    {s.output} (unavailable)
                  </option>
                ) : null}
              </select>
            </FormGroup>
            <FormGroup label="Layer">
              <select
                value={s.layer}
                aria-label={`Screen ${s.name} layer`}
                onChange={(e) => void run(() => patch(`/api/screens/${s.id}`, { layer: e.target.value }))}
              >
                {(outputs.find((o) => o.output_id === s.output)?.layers ?? [s.layer]).map((l) => (
                  <option key={l} value={l}>{l}</option>
                ))}
              </select>
            </FormGroup>
            <FormGroup label="Usually rendered by">
              {/* The TEMPLATE, never a revision: the Item pins the revision when
                  it is saved, or publishing would silently change every graphic
                  that goes here (ADR 0023). */}
              <select
                value={s.template_id ?? ''}
                aria-label={`Screen ${s.name} template`}
                onChange={(e) =>
                  void run(() =>
                    patch(`/api/screens/${s.id}`, { template_id: e.target.value || null }),
                  )
                }
              >
                <option value="">The Item&rsquo;s own template</option>
                {templates.map((t) => <option key={t.id} value={t.id}>{t.name}</option>)}
              </select>
            </FormGroup>
            <button
              type="button"
              className="edit-console__iconbtn"
              aria-label={`Delete screen ${s.name}`}
              onClick={() => void run(() => del(`/api/screens/${s.id}`))}
            >
              <Trash2 size={13} aria-hidden />
            </button>
          </div>
        </div>
      ))}

      {problem ? <p className="tl__problem" role="alert">{problem}</p> : null}

      <div className="cfg-console__screen-add">
        <input
          placeholder="name, e.g. LED Wall"
          aria-label="New screen name"
          value={name}
          onChange={(e) => setName(e.target.value)}
        />
        <button
          type="button"
          className="edit-console__addbtn"
          disabled={!name.trim() || outputs.length === 0}
          title={outputs.length === 0 ? 'Adopt a node before creating a screen' : undefined}
          onClick={() => {
            const first = outputs[0]!
            void run(async () => {
              await post(keys.screens(), {
                name: name.trim(),
                node_id: first.node_id,
                output: first.output_id,
                layer: first.layers[0] ?? 'Graphics',
              })
              setName('')
            })
          }}
        >
          <Plus size={12} aria-hidden /> Add a screen
        </button>
      </div>
    </section>
  )
}

function Stranger({ node }: { node: FullNode }) {
  const [busy, setBusy] = useState(false)
  const adopt = useCallback(async () => {
    setBusy(true)
    try {
      await post(`${keys.node(node.node_id)}/adopt`, { adopted: true })
      invalidate(keys.nodes())
    } finally {
      setBusy(false)
    }
  }, [node.node_id])

  return (
    <div className="cfg-console__stranger">
      <div className="cfg-console__stranger-text">
        <span className="cfg-console__stranger-name">
          {node.name}
          <span className="cfg-console__mono">
            {'  ·  '}{node.node_id.slice(0, 10)}…{'  ·  '}v{node.version ?? '?'}
          </span>
        </span>
        <span className="cfg-console__dim">
          This node is asking to join. Until it is adopted it receives no templates, no items,
          and no cues.
        </span>
      </div>
      <Button onClick={() => void adopt()} disabled={busy}>
        <Check size={15} aria-hidden /> {busy ? 'Adopting…' : 'Adopt'}
      </Button>
    </div>
  )
}

function AdoptedNode({ node }: { node: FullNode }) {
  const { data } = useQuery<{
    outputs: { output_id: string; mode: string; layers: string[]; color?: ColorConfig }[]
  }>(`${keys.node(node.node_id)}/outputs`)
  const caps = node.capabilities
  const outputs = data?.outputs ?? []
  const liveOutputs = node.state?.outputs ?? []

  return (
    <div className="cfg-console__node">
      <NodeCard
        name={node.name}
        role={node.reachable ? undefined : 'OFFLINE'}
        healthy={node.reachable}
        status={
          node.reachable
            ? undefined
            : `${node.name} is offline. Anything already on air is still on air.`
        }
        metrics={[
          { label: 'Version', value: `v${node.version ?? '?'}` },
          { label: 'GPU', value: caps?.gpu ? `${caps.gpu.adapter} (${caps.gpu.backend})` : 'not reported' },
          {
            label: 'Devices',
            value: caps?.outputs?.length
              ? caps.outputs.map((o) => `${o.device}${o.supports_key_fill ? ' · key+fill' : ''}`).join(', ')
              : 'No hardware list (NDI)',
          },
          { label: 'Max layers', value: String(caps?.max_layers ?? '?'), accent: true },
        ]}
      />

      <OutputHardware node={node} />

      {/* What somebody needs when it is going wrong: where the node is, what
          it is running, whether it has the content we think it has, and how
          many frames it has been late for. Every number here was already
          reported and none of it was shown. */}
      <details className="cfg-console__diag">
        <summary>Diagnostics</summary>
        <div className="cfg-console__diaggrid">
          <KeyValueRow label="Address" value={node.address ?? 'not connected yet'} tone="plain" />
          <KeyValueRow
            label="Node id"
            value={node.node_id}
            tone="plain"
          />
          <KeyValueRow
            label="Assignment"
            value={
              node.assignment_gen == null
                ? 'nothing acked yet'
                : `gen ${node.assignment_gen} acked`
            }
            tone="plain"
          />
          <KeyValueRow
            label="Last seen"
            value={node.last_seen ? new Date(node.last_seen).toLocaleTimeString() : 'never'}
            tone="plain"
          />
          <KeyValueRow
            label="Scene reader"
            value={caps?.scene_reader_major ? `v${caps.scene_reader_major}` : 'not reported'}
            tone="plain"
          />
          <KeyValueRow
            label="Decoders"
            value={caps?.decoders?.length ? caps.decoders.join(', ') : 'not reported'}
            tone="plain"
          />
        </div>

        {liveOutputs.length ? (
          <div className="cfg-console__diaggrid">
            {liveOutputs.map((o) => (
              <KeyValueRow
                key={o.id}
                label={`${o.id} · frames`}
                // A late frame is the number that matters here, so it is the
                // one that gets the accent the moment there is one.
                value={`${o.fps?.toFixed(2) ?? '?'} fps · ${o.late_frames ?? 0} late`}
                tone={o.late_frames ? 'armed' : 'plain'}
              />
            ))}
          </div>
        ) : null}

        <p className="cfg-console__note">
          {node.status_url ? (
            <>
              The node answers for itself at{' '}
              <a href={node.status_url} target="_blank" rel="noreferrer">
                {node.status_url}
              </a>. This page remains available if the control server is offline.
            </>
          ) : (
            'This node has not connected yet, so there is nowhere to point at.'
          )}
        </p>
      </details>

      {liveOutputs.some((o) => o.sync) ? (
        <div className="cfg-console__refs">
          {liveOutputs.filter((o) => o.sync).map((o) => (
            <div key={o.id} className="cfg-console__ref">
              <span className="cfg-console__mono">Reference{'  ·  '}{o.id}</span>
              {o.sync?.locked ? (
                <StatusPill tone="ready" variant="subtle" size={10}>
                  {`LOCKED · ${(o.sync.format || 'unknown format').toUpperCase()}`}
                </StatusPill>
              ) : o.sync?.present ? (
                // Present but not locked is the interesting state: a reference
                // is connected and the card is not following it, which usually
                // means a format mismatch.
                <StatusPill tone="staged" variant="subtle" size={10}>
                  {`PRESENT, NOT LOCKED${o.sync.format ? ` · ${o.sync.format.toUpperCase()}` : ''}`}
                </StatusPill>
              ) : (
                <StatusPill tone="muted" variant="subtle" size={10}>
                  NO REFERENCE · FREE RUNNING
                </StatusPill>
              )}
            </div>
          ))}
        </div>
      ) : null}

      {(outputs.length
        ? outputs
        : liveOutputs.map((o) => ({ output_id: o.id, mode: '', layers: [], color: undefined }))
      ).map((o) => (
        <OutputEditor
          key={o.output_id}
          nodeId={node.node_id}
          outputId={o.output_id}
          mode={o.mode}
          color={o.color}
          layers={o.layers.length ? o.layers : (liveOutputs.find((l) => l.id === o.output_id)?.layers ?? []).map((l) => l.name)}
        />
      ))}
    </div>
  )
}

/**
 * The layer stack, bottom first.
 *
 * That order **is** the compositing order, so the arrows are not cosmetic —
 * moving a layer up puts it in front of the one it passes. Layers are addressed
 * by name everywhere else in the system, so renaming one here would orphan
 * every Item pointing at it; there is deliberately no rename.
 */
function OutputEditor({
  nodeId,
  outputId,
  mode,
  layers,
  color,
}: {
  nodeId: string
  outputId: string
  mode: string
  layers: string[]
  color?: ColorConfig
}) {
  const [draft, setDraft] = useState<string[]>(layers)
  const [col, setCol] = useState<ColorConfig>(
    color ?? { matrix: 'bt709', range: 'narrow', key_range: 'narrow', key_mode: 'fill' },
  )
  const [pattern, setPattern] = useState<string | null>(null)
  const [adding, setAdding] = useState('')
  const [problem, setProblem] = useState<string | null>(null)
  const [saved, setSaved] = useState(false)

  const save = useCallback(
    async (next: string[]) => {
      setProblem(null)
      try {
        await put(`${keys.node(nodeId)}/outputs/${outputId}`, {
          layers: next,
          mode: mode || '1920x1080',
          color: col,
        })
        setDraft(next)
        setSaved(true)
        invalidate(`${keys.node(nodeId)}/outputs`)
        invalidate(keys.nodes())
        setTimeout(() => setSaved(false), 1500)
      } catch (e) {
        setProblem(e instanceof ApiError ? e.message : String(e))
      }
    },
    [nodeId, outputId, mode, col],
  )

  const move = (i: number, delta: number) => {
    const to = i + delta
    if (to < 0 || to >= draft.length) return
    const next = [...draft]
    const [x] = next.splice(i, 1)
    next.splice(to, 0, x as string)
    void save(next)
  }

  return (
    <div className="cfg-console__output">
      <div className="cfg-console__output-head">
        <span className="cfg-console__output-name">
          {outputId} <span className="cfg-console__mono">{mode || '1920x1080'}</span>
        </span>
        {saved ? (
          <StatusPill tone="ready" variant="subtle" size={10}>PUSHED TO THE NODE</StatusPill>
        ) : null}
      </div>
      <p className="cfg-console__dim">
        Bottom of the stack first. What is lower is covered by what is above it.
      </p>

      {problem ? (
        <p className="cfg-console__problem" role="alert">{problem}</p>
      ) : null}

      <div className="cfg-console__layers">
        {draft.map((name, i) => (
          <div key={name} className="cfg-console__layer">
            <span className="cfg-console__mono">{String(i + 1).padStart(2, '0')}</span>
            <span className="cfg-console__layer-name">{name}</span>
            <button
              type="button"
              className="edit-console__iconbtn"
              aria-label={`Move ${name} down the stack`}
              disabled={i === 0}
              onClick={() => move(i, -1)}
            >
              <ChevronDown size={13} aria-hidden /> back
            </button>
            <button
              type="button"
              className="edit-console__iconbtn"
              aria-label={`Move ${name} up the stack`}
              disabled={i === draft.length - 1}
              onClick={() => move(i, 1)}
            >
              <ChevronUp size={13} aria-hidden /> front
            </button>
            <button
              type="button"
              className="edit-console__iconbtn"
              aria-label={`Remove ${name}`}
              onClick={() => void save(draft.filter((l) => l !== name))}
            >
              <X size={13} aria-hidden />
            </button>
          </div>
        ))}
      </div>

      <div className="cfg-console__grid">
        <div className="cfg-console__fieldset">
          <GroupHeader label="Colour" />
          {([
            ['matrix', 'Matrix', ['bt709', 'bt601', 'bt2020']],
            ['range', 'Fill range', ['narrow', 'full']],
            // Separate from the fill range on purpose: a narrow fill with a
            // full key is a real plant configuration.
            ['key_range', 'Key range', ['narrow', 'full']],
            ['key_mode', 'Key mode', ['fill', 'key', 'fill_key', 'embedded']],
          ] as const).map(([field, label, options]) => (
            <FormGroup key={field} label={label}>
              <select
                value={col[field]}
                onChange={(e) => {
                  const next = { ...col, [field]: e.target.value }
                  setCol(next)
                  void save(draft)
                }}
              >
                {options.map((o) => (
                  <option key={o} value={o}>{o}</option>
                ))}
              </select>
            </FormGroup>
          ))}
        </div>

        <div className="cfg-console__fieldset">
          <GroupHeader label="Test patterns" />
          <p className="cfg-console__dim">
            These sit above the layer stack, so you can prove a configuration without clearing
            the show off air.
          </p>
          <div className="cfg-console__patterns">
            {PATTERNS.map((p) => (
              <Button
                key={p.id}
                variant={pattern === p.id ? 'armed' : 'secondary'}
                title={`Proves ${p.proves}`}
                onClick={() => {
                  const next = pattern === p.id ? null : p.id
                  setPattern(next)
                  void post(`${keys.node(nodeId)}/outputs/${outputId}/test-pattern`, { pattern: next })
                }}
              >
                {p.label}
              </Button>
            ))}
          </div>
          {pattern ? (
            <p className="cfg-console__dim">{PATTERNS.find((p) => p.id === pattern)?.proves}</p>
          ) : null}
        </div>
      </div>

      <form
        className="cfg-console__add"
        onSubmit={(e) => {
          e.preventDefault()
          const name = adding.trim()
          if (!name) return
          setAdding('')
          void save([...draft, name])
        }}
      >
        <input
          value={adding}
          onChange={(e) => setAdding(e.target.value)}
          placeholder="New layer name"
          aria-label="New layer name"
        />
        <button type="submit" className="c-btn c-btn--secondary">
          <Plus size={13} aria-hidden /> Add
        </button>
      </form>
    </div>
  )
}
