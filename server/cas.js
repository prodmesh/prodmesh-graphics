/**
 * The content-addressed store. Spec: `docs/specs/asset-cas.md`. ADR 0006.
 *
 * The server is the origin; nodes pull what they are missing and cache it
 * forever. Every byte is named `blake3:<64 hex>` of its exact contents, which
 * is what makes "does this node have the right font?" answerable rather than
 * hopeful.
 *
 * # Three rules, all load-bearing
 *
 * 1. **Writes are atomic.** Stream to `tmp/`, hash while writing, verify, then
 *    `rename(2)` into `blobs/`. Rename within a filesystem is atomic, so a
 *    partial file is never visible as complete. Without this a truncated
 *    download is indistinguishable from a good one until it renders wrong.
 * 2. **The hash is verified, never trusted.** A caller announcing a hash gets
 *    checked against the bytes it actually sent. The whole guarantee collapses
 *    if the address is taken on faith.
 * 3. **The cache is permanent, not LRU.** Deleting an asset to reclaim disk is
 *    an operator action that reports before it removes. A store that silently
 *    evicts turns a disk warning into an on-air failure an hour later.
 *
 * # Why hash-wasm rather than a native addon
 *
 * blake3 is not in Node's `crypto`. `hash-wasm` is a pure-WASM implementation
 * with no node-gyp step, so a booth machine restoring this app does not need a
 * compiler. Verified against the Rust `blake3` crate in cas.test.js — the two
 * must agree exactly or content addressing means nothing across the boundary.
 */

import { createBLAKE3 } from 'hash-wasm'
import fs from 'node:fs'
import fsp from 'node:fs/promises'
import path from 'node:path'
import { randomBytes } from 'node:crypto'

const PREFIX = 'blake3:'
const HASH_RE = /^blake3:[0-9a-f]{64}$/

let root = null

export function init(dir = process.env.PMG_CAS ?? 'var/cas') {
  root = path.resolve(dir)
  fs.mkdirSync(path.join(root, 'blobs'), { recursive: true })
  fs.mkdirSync(path.join(root, 'tmp'), { recursive: true })
  return root
}

function base() {
  return root ?? init()
}

export function isHash(h) {
  return typeof h === 'string' && HASH_RE.test(h)
}

/**
 * Two-level fan-out on the first two bytes, so no directory ever holds a
 * pathological number of entries: `blobs/9f/2c/9f2c8a…`.
 */
export function blobPath(hash) {
  if (!isHash(hash)) throw new Error(`not a content address: ${hash}`)
  const hex = hash.slice(PREFIX.length)
  return path.join(base(), 'blobs', hex.slice(0, 2), hex.slice(2, 4), hex)
}

export async function hashBuffer(buf) {
  const h = await createBLAKE3()
  h.init()
  h.update(buf)
  return PREFIX + h.digest('hex')
}

/**
 * Store bytes and return their address.
 *
 * @param {Buffer} buf
 * @param {string} [expect] If given, the caller's claimed address. A mismatch
 *   throws rather than storing under the true hash, because a caller whose
 *   bytes do not match its claim has a bug the store must not paper over.
 */
export async function put(buf, expect) {
  const hash = await hashBuffer(buf)
  if (expect && expect !== hash) {
    const e = new Error(`content address mismatch: claimed ${expect}, computed ${hash}`)
    e.code = 'hash_mismatch'
    throw e
  }

  const dest = blobPath(hash)
  // Already present is success, not a conflict — that is the whole point of
  // addressing by content.
  if (fs.existsSync(dest)) return { hash, size: buf.length, deduped: true }

  const tmp = path.join(base(), 'tmp', randomBytes(16).toString('hex'))
  await fsp.mkdir(path.dirname(dest), { recursive: true })
  await fsp.writeFile(tmp, buf)
  await fsp.rename(tmp, dest)
  return { hash, size: buf.length, deduped: false }
}

export function has(hash) {
  return isHash(hash) && fs.existsSync(blobPath(hash))
}

export async function get(hash) {
  return fsp.readFile(blobPath(hash))
}

export function stat(hash) {
  try {
    return fs.statSync(blobPath(hash))
  } catch {
    return null
  }
}

/**
 * `GET /cas/:hash` — with Range and resume, which are **required, not optional**.
 * A node interrupted at 3.9 GB of a 4 GB video must not start over.
 *
 * Content addresses are immutable, so `immutable` caching is exactly correct
 * here and a large installation can front this with a caching proxy without any
 * node knowing.
 */
export function handleGet(req, res) {
  const hash = req.params.hash
  if (!isHash(hash)) {
    return res.status(400).json({ error: 'validation_failed', label: 'That is not a content address.' })
  }
  const st = stat(hash)
  if (!st) {
    return res.status(404).json({ error: 'not_found', label: 'This server does not have that asset.' })
  }

  res.setHeader('Accept-Ranges', 'bytes')
  res.setHeader('ETag', `"${hash}"`)
  res.setHeader('Cache-Control', 'public, max-age=31536000, immutable')
  res.setHeader('Content-Type', 'application/octet-stream')

  const range = req.headers.range
  if (!range) {
    res.setHeader('Content-Length', st.size)
    if (req.method === 'HEAD') return res.status(200).end()
    return fs.createReadStream(blobPath(hash)).pipe(res)
  }

  // Only `bytes=<start>-<end?>` — the resume case. Multi-range exists in the
  // RFC and no client of ours sends it; answering 416 is honest, whereas
  // pretending to satisfy it by returning the first range is not.
  const m = /^bytes=(\d+)-(\d*)$/.exec(range.trim())
  if (!m) {
    res.setHeader('Content-Range', `bytes */${st.size}`)
    return res.status(416).end()
  }
  const start = Number(m[1])
  const end = m[2] === '' ? st.size - 1 : Math.min(Number(m[2]), st.size - 1)
  if (start >= st.size || end < start) {
    res.setHeader('Content-Range', `bytes */${st.size}`)
    return res.status(416).end()
  }

  res.status(206)
  res.setHeader('Content-Range', `bytes ${start}-${end}/${st.size}`)
  res.setHeader('Content-Length', end - start + 1)
  if (req.method === 'HEAD') return res.end()
  return fs.createReadStream(blobPath(hash), { start, end }).pipe(res)
}

/**
 * Re-hash every blob and report mismatches.
 *
 * Three mismatches is not a statistic, it is a fault: something is corrupting
 * data in transit or at rest, and it should be loud rather than logged.
 */
export async function verify() {
  const blobs = path.join(base(), 'blobs')
  const bad = []
  let checked = 0
  for (const a of await fsp.readdir(blobs).catch(() => [])) {
    for (const b of await fsp.readdir(path.join(blobs, a)).catch(() => [])) {
      for (const f of await fsp.readdir(path.join(blobs, a, b)).catch(() => [])) {
        checked++
        const buf = await fsp.readFile(path.join(blobs, a, b, f))
        const actual = await hashBuffer(buf)
        if (actual !== PREFIX + f) bad.push({ expected: PREFIX + f, actual })
      }
    }
  }
  return { checked, bad }
}

/** Test seam. */
export function _reset() {
  root = null
}
