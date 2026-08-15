/**
 * Media ingest: probe, judge, and transcode to an all-intra intermediate.
 *
 * # Why this exists at all
 *
 * **Long-GOP H.264 is where frame-accurate seek goes to die.** A frame in the
 * middle of a 250-frame GOP cannot be produced without decoding everything back
 * to the last keyframe, so "start this clip at the mark" becomes "start it
 * somewhere near the mark, after a pause". Requiring an all-intra intermediate
 * — ProRes, DNxHR, HAP — is the single decision that removes most of the media
 * risk in R7, and doing the conversion **on ingest** is what stops a booth
 * machine discovering the problem on Sunday.
 *
 * # The rejection has to be a sentence
 *
 * A file that cannot be used says why, in terms of the file: what codec it is,
 * what is wrong with it, and what to do. "Unsupported format" sends someone to
 * a forum; "This is long-GOP H.264, which cannot seek accurately — it will be
 * converted to ProRes" tells them what is happening to their file.
 *
 * # Jobs, not requests
 *
 * A transcode of a service-length clip takes minutes. It runs as a job with a
 * status endpoint rather than holding an HTTP request open, and **one at a
 * time** — parallel transcodes on a booth machine would compete with the thing
 * actually going on air.
 */

import { spawn } from 'node:child_process'
import fs from 'node:fs'
import fsp from 'node:fs/promises'
import os from 'node:os'
import path from 'node:path'
import * as cas from './cas.js'
import { handle } from './db.js'
import { newId } from './ids.js'

/** Codecs we will play as-is: all-intra, seekable frame by frame. */
const ALL_INTRA = new Set(['prores', 'dnxhd', 'hap', 'qtrle', 'rawvideo', 'v210', 'mjpeg', 'ffv1'])

/**
 * Codecs we can read but must convert.
 *
 * Not a blocklist of bad formats — these are perfectly good delivery codecs.
 * They are simply not seekable a frame at a time, which is what playout needs.
 */
const NEEDS_TRANSCODE = new Set(['h264', 'hevc', 'vp9', 'av1', 'mpeg4', 'mpeg2video', 'vp8', 'theora'])

const jobs = new Map()
let running = false

const run = (cmd, args) =>
  new Promise((resolve) => {
    const p = spawn(cmd, args)
    let out = '', err = ''
    p.stdout.on('data', (d) => { out += d })
    p.stderr.on('data', (d) => { err += d })
    p.on('error', (e) => resolve({ code: -1, out, err: e.message }))
    p.on('close', (code) => resolve({ code, out, err }))
  })

/**
 * What ffprobe says about a file.
 *
 * Returns `null` when the file is not media at all, which is a different
 * failure from "media we cannot use" and gets a different message.
 */
export async function probe(file) {
  const r = await run('ffprobe', [
    '-v', 'error', '-print_format', 'json',
    '-show_streams', '-show_format', file,
  ])
  if (r.code !== 0) return null
  let parsed
  try {
    parsed = JSON.parse(r.out)
  } catch {
    return null
  }
  const v = (parsed.streams ?? []).find((s) => s.codec_type === 'video')
  if (!v) return null

  const [num, den] = String(v.r_frame_rate ?? '0/1').split('/').map(Number)
  return {
    codec: v.codec_name,
    width: v.width,
    height: v.height,
    fps: den ? num / den : 0,
    fps_num: num,
    fps_den: den,
    duration_s: Number(parsed.format?.duration ?? v.duration ?? 0),
    // pix_fmt carrying an alpha plane is the only reliable signal; a container
    // claiming alpha it does not have is common.
    has_alpha: /a$|yuva|rgba|argb|bgra/i.test(v.pix_fmt ?? ''),
    pix_fmt: v.pix_fmt,
    audio: (parsed.streams ?? []).filter((s) => s.codec_type === 'audio').length,
  }
}

/**
 * Decide what to do with a probed file.
 *
 * Three outcomes, and the difference matters: **accept** it untouched, **convert**
 * it and say why, or **reject** it because nothing we can do will make it
 * playable.
 */
export function judge(info) {
  if (!info) {
    return { action: 'reject', reason: 'This file does not contain video that ffmpeg can read.' }
  }
  if (!info.width || !info.height) {
    return { action: 'reject', reason: 'This file has no usable video stream.' }
  }
  if (ALL_INTRA.has(info.codec)) {
    return {
      action: 'accept',
      reason: `${info.codec} is all-intra and seeks frame by frame. Stored as-is.`,
    }
  }
  if (NEEDS_TRANSCODE.has(info.codec)) {
    return {
      action: 'convert',
      // Named in terms of the file and what happens next, not "unsupported".
      reason:
        `${info.codec} is long-GOP, which cannot seek to an exact frame — a clip ` +
        `told to start on a mark would start near it, after a pause. Converting to ` +
        `${info.has_alpha ? 'ProRes 4444 (alpha preserved)' : 'ProRes 422 HQ'}.`,
    }
  }
  return {
    action: 'convert',
    reason: `${info.codec} is not a playout codec. Converting to ProRes for frame-accurate seek.`,
  }
}

/** ProRes 4444 when there is alpha to keep, 422 HQ when there is not. */
function transcodeArgs(src, dst, info) {
  return [
    '-hide_banner', '-y', '-i', src,
    '-c:v', 'prores_ks',
    // 4444 carries alpha; 3 is 422 HQ. Choosing 4444 for everything would
    // roughly double the file for an alpha channel that does not exist.
    '-profile:v', info.has_alpha ? '4' : '3',
    ...(info.has_alpha ? ['-pix_fmt', 'yuva444p10le'] : ['-pix_fmt', 'yuv422p10le']),
    // Audio to PCM: it travels with the clip and must not need its own decoder
    // on the frame path.
    '-c:a', 'pcm_s16le',
    dst,
  ]
}

export function get(id) {
  return jobs.get(id) ?? null
}

export function list() {
  return [...jobs.values()].map(({ proc, ...j }) => j)
}

/**
 * Start an ingest. Returns the job immediately; it runs in the background.
 */
export async function start({ bytes, filename = 'upload' }) {
  const id = newId()
  const job = {
    id,
    filename,
    state: 'probing',
    started_at: Date.now(),
    source_hash: null,
    hash: null,
    info: null,
    action: null,
    message: null,
  }
  jobs.set(id, job)

  const dir = await fsp.mkdtemp(path.join(os.tmpdir(), 'pmg-ingest-'))
  const src = path.join(dir, 'source')
  await fsp.writeFile(src, bytes)

  // Deliberately not awaited: the caller gets the job id straight back.
  void (async () => {
    try {
      const info = await probe(src)
      const verdict = judge(info)
      job.info = info
      job.action = verdict.action
      job.message = verdict.reason

      if (verdict.action === 'reject') {
        job.state = 'rejected'
        return
      }

      if (verdict.action === 'accept') {
        job.state = 'storing'
        const stored = await cas.put(await fsp.readFile(src))
        job.hash = stored.hash
        record(stored.hash, stored.size, info, filename)
        job.state = 'done'
        return
      }

      // Convert. One at a time — parallel transcodes on a booth machine would
      // compete with whatever is going on air.
      job.state = 'queued'
      while (running) await new Promise((r) => setTimeout(r, 250))
      running = true
      job.state = 'converting'

      const dst = path.join(dir, 'out.mov')
      const r = await run('ffmpeg', transcodeArgs(src, dst, info))
      running = false
      if (r.code !== 0 || !fs.existsSync(dst)) {
        job.state = 'failed'
        // The last few lines of ffmpeg's own output: it usually says exactly
        // what it could not do, and paraphrasing it loses that.
        job.message = `Conversion failed. ffmpeg said: ${r.err.trim().split('\n').slice(-3).join(' ')}`
        return
      }

      job.state = 'storing'
      const stored = await cas.put(await fsp.readFile(dst))
      job.hash = stored.hash
      const after = await probe(dst)
      record(stored.hash, stored.size, after ?? info, filename)
      job.info = after ?? info
      job.state = 'done'
    } catch (e) {
      running = false
      job.state = 'failed'
      job.message = String(e?.message ?? e)
    } finally {
      await fsp.rm(dir, { recursive: true, force: true }).catch(() => {})
    }
  })()

  return job
}

function record(hash, size, info, filename) {
  handle()
    .prepare(
      `INSERT INTO assets (hash, size, kind, first_seen, media)
       VALUES (?, ?, 'video', ?, ?)
       ON CONFLICT(hash) DO UPDATE SET media = excluded.media, kind = 'video'`,
    )
    .run(hash, size, Date.now(), JSON.stringify({ ...info, filename }))
}

/** Test seam. */
export function _reset() {
  jobs.clear()
  running = false
}
