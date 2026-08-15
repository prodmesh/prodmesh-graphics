/**
 * S8 — cue delivery under chaos.
 *
 * Drives a REAL control server and a REAL `pmg-node` while breaking things, and
 * asserts the two absolutes: **no take is lost, and no take fires twice.**
 *
 * # Why this is JavaScript and not `pmgctl chaos`
 *
 * The spike brief says `pmgctl chaos`. The server is plain-ESM Node, and the
 * harness has to *restart* it, so it has to own it as a subprocess — which puts
 * the harness on this side of the fence. Running it from Rust would mean Rust
 * spawning Node spawning nothing, with the interesting control (kill the
 * socket, blackhole it) still living here. Recorded as a deliberate deviation
 * rather than a silent one.
 *
 * # The proxy is the whole trick
 *
 *     pmg-node ──► [this proxy] ──► control server
 *
 * The node dials the proxy, so the harness can sever the control link at will
 * without touching the server. The proxy tells a **WebSocket upgrade** apart
 * from a plain HTTP asset fetch by sniffing the first request line, which is
 * what lets "kill the link" and "saturate the asset path" be different
 * injections — and that distinction is the point, because the three-connection
 * split in ADR 0007 exists precisely so the second cannot delay the first.
 */

import { spawn } from 'node:child_process'
import net from 'node:net'
import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'

const ROOT = path.join(import.meta.dirname, '..')
const SERVER_PORT = 8721
const PROXY_PORT = 8722
const NODE_NAME = 'chaos-1'
const LAYERS = ['Fullscreen', 'Lower Third', 'Bug']

const arg = (flag, dflt) => {
  const i = process.argv.indexOf(flag)
  return i >= 0 ? process.argv[i + 1] : dflt
}
const TAKES = Number(arg('--takes', 1000))
const FONT = arg('--font', '/System/Library/Fonts/Supplemental/Arial.ttf')
const QUIET = process.argv.includes('--quiet')

const log = (...a) => { if (!QUIET) console.log(...a) }
/** Always shown. A harness that can hang has to say where it got to. */
const step = (...a) => process.stderr.write(`[chaos] ${a.join(' ')}\n`)
const sleep = (ms) => new Promise((r) => setTimeout(r, ms))

// ---------------------------------------------------------------------------
// The severable link
// ---------------------------------------------------------------------------

/**
 * A TCP proxy that can be told to drop control connections or swallow them.
 *
 * `blackhole` accepts and then says nothing — which is the failure that matters,
 * because a socket that is merely *closed* reconnects immediately while one that
 * hangs is what produces zombie cues arriving after their deadline.
 */
function makeProxy() {
  const sockets = new Set()
  let mode = 'pass' // pass | blackhole

  const server = net.createServer((client) => {
    let upstream = null
    let buffered = []
    let isControl = null

    const close = () => {
      client.destroy()
      upstream?.destroy()
      sockets.delete(entry)
    }
    const entry = { client, get upstream() { return upstream }, get isControl() { return isControl }, close }
    sockets.add(entry)

    client.on('data', (chunk) => {
      if (isControl === null) {
        // First bytes carry the request line and headers. A control link is the
        // WebSocket upgrade on /link; anything else is the asset path.
        const head = chunk.toString('latin1', 0, 512)
        isControl = /^GET \/link/m.test(head) || /upgrade:\s*websocket/i.test(head)
        if (isControl && mode === 'blackhole') {
          // Accept and say nothing. The node sits waiting rather than failing
          // fast, which is what generates cues that arrive after their deadline.
          buffered = []
          return
        }
        upstream = net.connect(SERVER_PORT, '127.0.0.1')
        upstream.on('data', (d) => client.write(d))
        upstream.on('error', close)
        upstream.on('close', close)
        for (const b of buffered) upstream.write(b)
        buffered = []
      }
      if (upstream) upstream.write(chunk)
      else buffered.push(chunk)
    })
    client.on('error', close)
    client.on('close', close)
  })

  return {
    listen: () => new Promise((r) => server.listen(PROXY_PORT, '127.0.0.1', r)),
    /** Sever every control connection. Asset transfers are left alone. */
    killControl() {
      let n = 0
      for (const s of sockets) if (s.isControl) { s.close(); n++ }
      return n
    },
    blackhole(on) {
      mode = on ? 'blackhole' : 'pass'
      if (on) this.killControl()
    },
    close: () => new Promise((r) => { for (const s of sockets) s.close(); server.close(r) }),
  }
}

// ---------------------------------------------------------------------------
// Processes
// ---------------------------------------------------------------------------

const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'pmg-chaos-'))
const DB = path.join(tmp, 'chaos.db')
const CAS = path.join(tmp, 'cas')
const NODE_STATE = path.join(tmp, 'node')

let serverProc = null
let nodeProc = null

function startServer() {
  serverProc = spawn('node', [path.join(ROOT, 'server/index.js')], {
    env: { ...process.env, PORT: String(SERVER_PORT), PMG_DB: DB, PMG_CAS: CAS },
    stdio: ['ignore', 'pipe', 'pipe'],
  })
  serverProc.stderr.on('data', (d) => { if (!QUIET) process.stderr.write(`[server] ${d}`) })
  return waitFor(() => api('GET', '/api/version').then(() => true).catch(() => false), 10_000, 'server')
}

function startNode() {
  nodeProc = spawn(
    path.join(ROOT, 'target/release/pmg-node'),
    ['--server', `127.0.0.1:${PROXY_PORT}`, '--state-dir', NODE_STATE, '--name', NODE_NAME,
     '--headless', '--font', FONT, '--layers', LAYERS.join(',')],
    { stdio: ['ignore', 'pipe', 'pipe'] },
  )
  nodeProc.stderr.on('data', (d) => { if (!QUIET) process.stderr.write(`[node] ${d}`) })
  return waitFor(async () => {
    const n = await nodes()
    if (!n?.reachable) return false
    // Adopt it. A node nobody has adopted is deliberately given nothing, so
    // without this the harness would measure the adoption gate rather than the
    // cue path.
    if (!n.adopted) await api('POST', `/api/nodes/${n.node_id}/adopt`, { adopted: true })
    return true
  }, 15_000, 'node link')
}

const stop = (p) => new Promise((r) => {
  if (!p || p.exitCode !== null) return r()
  p.once('exit', r)
  p.kill('SIGKILL')
})

async function waitFor(pred, ms, what) {
  const until = Date.now() + ms
  while (Date.now() < until) {
    try { if (await pred()) return true } catch { /* keep waiting */ }
    await sleep(100)
  }
  throw new Error(`timed out waiting for ${what}`)
}

// ---------------------------------------------------------------------------
// API
// ---------------------------------------------------------------------------

async function api(method, url, body, timeoutMs = 5000) {
  const ctl = AbortSignal.timeout(timeoutMs)
  const res = await fetch(`http://127.0.0.1:${SERVER_PORT}${url}`, {
    method,
    headers: body ? { 'content-type': 'application/json' } : {},
    ...(body ? { body: JSON.stringify(body) } : {}),
    signal: ctl,
  })
  const text = await res.text()
  return { status: res.status, json: text ? JSON.parse(text) : null }
}

const nodes = async () => (await api('GET', '/api/nodes')).json?.nodes?.find((n) => n.name === NODE_NAME)

// ---------------------------------------------------------------------------
// Seeding
// ---------------------------------------------------------------------------

async function seed() {
  const manifest = JSON.parse(fs.readFileSync(path.join(ROOT, 'testdata/templates/lower-third.manifest.json'), 'utf8'))
  const scene = fs.readFileSync(path.join(ROOT, 'testdata/templates/lower-third.pmgscene'))

  await fetch(`http://127.0.0.1:${SERVER_PORT}/api/assets?kind=font`, {
    method: 'POST',
    headers: { 'content-type': 'application/octet-stream' },
    body: fs.readFileSync(FONT),
  })
  // A big blob for the saturation injection. Random so it does not compress
  // away and actually occupies the link.
  const big = Buffer.alloc(8 * 1024 * 1024)
  for (let i = 0; i < big.length; i += 4096) big.writeUInt32LE((Math.random() * 2 ** 32) >>> 0, i)
  const bigRes = await fetch(`http://127.0.0.1:${SERVER_PORT}/api/assets?kind=blob`, {
    method: 'POST', headers: { 'content-type': 'application/octet-stream' }, body: big,
  })
  const bigHash = (await bigRes.json()).hash

  await api('POST', '/api/templates', { id: manifest.template_id, name: manifest.name })
  await api('POST', `/api/templates/${manifest.template_id}/publish`, {
    scene: scene.toString('base64'), manifest,
  })

  const items = []
  for (const layer of ['Lower Third', 'Bug', 'Fullscreen']) {
    for (let i = 0; i < 3; i++) {
      const r = await api('POST', '/api/items', {
        template_id: manifest.template_id,
        label: `${layer} ${i}`,
        data: { name: `${layer} ${i}`, title: 'chaos' },
        destination: { layer },
      })
      items.push(r.json)
    }
  }
  return { items, bigHash }
}

// ---------------------------------------------------------------------------
// The run
// ---------------------------------------------------------------------------

/** Everything we asked for, and what came back. One row per POST. */
const intents = []

async function fire(cueId, item, layer, op = 'take') {
  const t0 = performance.now()
  let outcome
  try {
    const r = await api('POST', '/api/cue', {
      cue_id: cueId, node: NODE_NAME, output: 'out-0', layer,
      op, item: item?.id, surface: 'api',
    }, 4000)
    outcome = {
      http: r.status,
      result: r.json?.result ?? null,
      error: r.json?.error ?? null,
    }
  } catch (e) {
    // A request that never came back at all. Recorded as its own outcome
    // rather than dropped — "we do not know" is a finding, not a gap.
    outcome = { http: 0, result: null, error: `transport: ${e.message}` }
  }
  const row = { cue_id: cueId, item: item?.id ?? null, layer, op, ms: performance.now() - t0, ...outcome }
  intents.push(row)
  return row
}

async function run() {
  log(`chaos: ${TAKES} takes, server :${SERVER_PORT}, node via proxy :${PROXY_PORT}`)
  const proxy = makeProxy()
  await proxy.listen()
  step('starting server')
  await startServer()
  step('seeding')
  const { items, bigHash } = await seed()
  step('starting node')
  await startNode()
  step(`seeded ${items.length} items; node linked`)

  const byLayer = (l) => items.filter((i) => i.destination.layer === l)
  const events = []
  const saturationLatencies = []
  let fired = 0
  let dedupeProven = 0
  let cameUpBlack = true
  const applied = new Map() // cue_id -> times the node said "applied"
  const lastPerLayer = new Map()

  // Chaos, on its own clock, while takes are firing.
  //
  // Gated on the take COUNT, and the take loop is not allowed to finish until
  // every injection has run. The first version keyed injections to a fraction
  // of progress and let the take loop end the run — and because a blackholed
  // link makes every take fail in about a millisecond, a 1000-take run drained
  // to completion *during* the 30 s blackhole and shut chaos down before the
  // server restart and the node restart ever fired. It reported PASS having
  // skipped the two hardest injections in the brief. A harness that can skip
  // its own hard cases and still go green is worse than no harness.
  let stopChaos = false
  let injectionsLeft = 5
  const chaos = (async () => {
    const injections = [
      { name: 'kill the control link mid-cue', at: 0.10, run: async () => { events.push(`killed ${proxy.killControl()} control socket(s)`) } },
      { name: 'blackhole the link for 30 s', at: 0.22, run: async () => {
          proxy.blackhole(true); await sleep(30_000); proxy.blackhole(false)
          events.push('blackholed the control link for 30 s')
        } },
      { name: 'restart the server', at: 0.50, run: async () => {
          await stop(serverProc); await startServer()
          // A restarted server has forgotten nothing that matters — adoption is
          // in the database — but the node must be re-adopted in this harness
          // only because adoption is per-node_id and the id is stable, so this
          // is a no-op that documents the expectation.
          events.push('restarted the control server')
        } },
      { name: 'kill the control link again', at: 0.62, run: async () => { events.push(`killed ${proxy.killControl()} control socket(s)`) } },
      { name: 'restart the node', at: 0.75, run: async () => {
          await stop(nodeProc)
          await startNode()

          // **It must come up black.** Auto-restore is a deliberate
          // non-feature: an unattended machine re-firing a graphic after a
          // power cut is a bad surprise (ADR 0009). Checked here, before any
          // new take, because a moment later it is unobservable.
          const fresh = await nodes()
          const occupied = (fresh?.state?.outputs?.[0]?.layers ?? []).filter((l) => l.occupant)
          if (occupied.length) {
            cameUpBlack = false
            events.push(`NODE DID NOT COME UP BLACK: ${occupied.map((l) => l.name).join(', ')}`)
          } else {
            events.push('restarted the node; it came up black, as designed')
          }

          // Intent resets with it. Everything taken before the restart is gone
          // BY DESIGN, so carrying it forward would assert the opposite of the
          // guarantee just verified — which is what the first version of this
          // harness did, and it reported a protocol failure that was its own.
          lastPerLayer.clear()
        } },
    ]
    for (const inj of injections) {
      // Both conditions are set by the take loop running concurrently; this
      // IS the wait for it. (The lint that flags this cannot see across the
      // two async tasks, so the rule is off in .oxlintrc.json.)
      while (!stopChaos && fired < Math.floor(TAKES * inj.at)) await sleep(50)
      if (stopChaos) { events.push(`SKIPPED: ${inj.name}`); injectionsLeft--; continue }
      log(`  ⚡ ${inj.name} (after ${fired} takes)`)
      await inj.run()
      injectionsLeft--
    }
  })()

  // Saturate the asset path while takes fire, for the property the
  // three-connection split exists to guarantee.
  // Saturation runs in a SEPARATE PROCESS.
  //
  // It used to run here, buffering each 8 MB pull with `arrayBuffer()` on the
  // same event loop that fires the takes and measures their latency. That
  // measures the harness starving itself, not the system under test: a third of
  // cue requests "timed out" at 4 s while the server was fine. The point is to
  // saturate the SERVER's asset path, so the load has to come from somewhere
  // that is not also the stopwatch.
  const saturator = spawn('node', ['-e', `
    const url = 'http://127.0.0.1:${SERVER_PORT}/cas/${bigHash}'
    let n = 0
    const pull = async () => {
      try {
        const r = await fetch(url)
        // Drain without accumulating: the bytes must cross the wire, they do
        // not need to be held.
        for await (const _ of r.body) { /* discard */ }
        process.stdout.write(String(++n) + '\\n')
      } catch { /* allowed to lose these */ }
      setTimeout(pull, 25)
    }
    pull(); pull()
  `], { stdio: ['ignore', 'pipe', 'ignore'] })
  let saturationPulls = 0
  saturator.stdout.on('data', (d) => { saturationPulls = Number(String(d).trim().split('\n').at(-1)) || saturationPulls })

  // Keep firing until the take budget is spent AND chaos has finished. Filler
  // takes past the budget are still real takes and still counted; what they buy
  // is that no injection ever runs against a silent system.
  const MAX_EXTRA = TAKES
  for (let i = 0; i < TAKES + MAX_EXTRA; i++) {
    if (i >= TAKES && injectionsLeft <= 0) break
    const layer = LAYERS[i % LAYERS.length]
    const pool = byLayer(layer)
    const item = pool[i % pool.length]
    const cueId = `chaos-${i}`

    const row = await fire(cueId, item, layer)
    if (row.result === 'applied') {
      applied.set(cueId, (applied.get(cueId) ?? 0) + 1)
      lastPerLayer.set(layer, item.id)
    }
    // Every take, applied or not. Percentiles over only the successful ones
    // would report a healthy p95 for a system that was timing out.
    if (saturationLatencies.length < 20000) saturationLatencies.push(row.ms)

    // Operator double-click: the SAME cue_id again, immediately. Must dedupe.
    //
    // Fired often, and deliberately also in the calm stretch before the first
    // injection — a run whose only double-clicks land while the link is down
    // proves nothing about idempotency, because every one of them is refused
    // for an unrelated reason.
    if (i % 7 === 3) {
      const again = await fire(cueId, item, layer)
      if (again.result === 'applied') applied.set(cueId, (applied.get(cueId) ?? 0) + 1)
      if (again.result === 'deduped') dedupeProven++
    }

    fired = i + 1
    if (fired % 100 === 0) step(`${fired}/${TAKES} (${injectionsLeft} injections left)`)
    // Paced. Without a floor here the loop outruns its own chaos schedule the
    // moment the link goes down and takes start failing in a millisecond.
    await sleep(25)
  }

  step(`takes done (${fired} fired); waiting for chaos to finish`)
  await chaos.catch((e) => events.push(`chaos aborted: ${e.message}`))
  stopChaos = true
  await chaos.catch((e) => events.push(`chaos aborted: ${e.message}`))
  events.push(`${saturationPulls} bulk asset pulls during the run`)

  // Let the link settle so the final state is the settled one, not a snapshot
  // taken mid-reconnect.
  step('quiescing')
  await waitFor(async () => (await nodes())?.reachable, 30_000, 'node to come back').catch(() => {})
  await sleep(3000)

  // A deterministic settle: one take per layer with the link healthy, and THAT
  // is what the final-state assertion checks.
  //
  // Without it the assertion is vacuous whenever the run ends with everything
  // rejected — which is exactly what happened on the first passing run, where
  // the node restart cleared the intent and nothing succeeded afterwards. A
  // green assertion over an empty expectation is not a check.
  step('settling')
  lastPerLayer.clear()
  for (const layer of LAYERS) {
    const pool = byLayer(layer)
    const item = pool[0]
    const row = await fire(`settle-${layer}`, item, layer)
    if (row.result === 'applied') lastPerLayer.set(layer, item.id)
    else events.push(`settle take for ${layer} did not apply: ${row.error ?? row.result}`)
    await sleep(150)
  }
  await sleep(1500)

  step('collecting verdict')
  return { items, events, applied, lastPerLayer, saturationLatencies, dedupeProven, cameUpBlack }
}

// ---------------------------------------------------------------------------
// Verdict
// ---------------------------------------------------------------------------

function percentile(xs, p) {
  if (!xs.length) return 0
  const s = [...xs].toSorted((a, b) => a - b)
  return s[Math.min(s.length - 1, Math.floor((p / 100) * s.length))]
}

async function verdict({ events, applied, lastPerLayer, saturationLatencies, dedupeProven, cameUpBlack }) {
  const node = await nodes()
  const live = node?.state?.outputs?.[0]?.layers ?? []

  const firedTwice = [...applied.entries()].filter(([, n]) => n > 1)
  const noOutcome = intents.filter((r) => r.result === null && r.error === null)
  const transportUnknown = intents.filter((r) => r.http === 0)
  const rejected = intents.filter((r) => r.http === 409 || r.http === 503)
  const appliedRows = intents.filter((r) => r.result === 'applied')
  const dedupedRows = intents.filter((r) => r.result === 'deduped')

  // The gap endpoint must return exactly what it says it will.
  const head = (await api('GET', '/api/cuelog?show=default&since=0')).json
  const from = Math.max(0, head.head_seq - 50)
  const gap = (await api('GET', `/api/cuelog?show=default&since=${from}`)).json
  const seqs = gap.entries.map((e) => e.seq)
  const gapOk =
    seqs.every((s, i) => i === 0 || s === seqs[i - 1] + 1) &&
    new Set(seqs).size === seqs.length &&
    seqs.every((s) => s > from)

  const report = {
    takes_attempted: intents.length,
    applied: appliedRows.length,
    deduped: dedupedRows.length,
    rejected: rejected.length,
    transport_unknown: transportUnknown.length,
    fired_twice: firedTwice.length,
    no_terminal_outcome: noOutcome.length,
    gap_recovery_ok: gapOk,
    latency_ms: {
      p50: +percentile(saturationLatencies, 50).toFixed(1),
      p95: +percentile(saturationLatencies, 95).toFixed(1),
      max: +Math.max(0, ...saturationLatencies).toFixed(1),
    },
    dedupes_observed: dedupeProven,
    node_came_up_black: cameUpBlack,
    final_state: live.map((l) => ({ layer: l.name, item: l.occupant?.item_id ?? null })),
    intended_final: [...lastPerLayer.entries()].map(([layer, item]) => ({ layer, item })),
    events,
  }

  // Compare, do not merely display. A harness that prints two lists and leaves
  // the reader to eyeball them is a harness that passes when nobody looks.
  const actual = new Map(live.map((l) => [l.name, l.occupant?.item_id ?? null]))
  const stateMismatch = [...lastPerLayer.entries()].filter(([layer, item]) => actual.get(layer) !== item)

  // The two absolutes.
  const failures = []
  if (firedTwice.length) failures.push(`${firedTwice.length} cue_id(s) applied more than once`)
  if (noOutcome.length) failures.push(`${noOutcome.length} cue(s) with no terminal outcome`)
  if (!gapOk) failures.push('cue-log gap recovery returned holes or duplicates')
  for (const [layer, item] of stateMismatch) {
    failures.push(`${layer}: on air is ${actual.get(layer) ?? 'nothing'}, last applied take was ${item}`)
  }
  if (!cameUpBlack) failures.push('the node restored graphics on boot instead of coming up black')
  if (lastPerLayer.size !== LAYERS.length) {
    // The settle phase is the only part of the run that is supposed to be
    // calm. If a take fails HERE, the system did not recover.
    failures.push(`settle phase: only ${lastPerLayer.size}/${LAYERS.length} layers took after recovery`)
  }
  if (appliedRows.length > 0 && dedupeProven === 0) {
    // Not a protocol failure — a coverage failure. A green run that never
    // exercised idempotency has not tested the thing most likely to break it.
    failures.push('no duplicate cue_id was ever deduped: idempotency went untested')
  }

  return { report, failures }
}

// ---------------------------------------------------------------------------

try {
  const result = await run()
  const { report, failures } = await verdict(result)
  console.log('\n--- S8 chaos ---')
  console.log(JSON.stringify(report, null, 2))
  if (failures.length) {
    console.error('\nFAIL')
    for (const f of failures) console.error(`  · ${f}`)
    process.exitCode = 1
  } else {
    console.log('\nPASS — no take fired twice, every cue reached a terminal outcome')
  }
} finally {
  await stop(nodeProc)
  await stop(serverProc)
  fs.rmSync(tmp, { recursive: true, force: true })
}
