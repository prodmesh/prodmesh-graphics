/**
 * Ingest: what gets accepted, what gets converted, and what gets refused.
 *
 * `judge()` is pure, so most of this needs no ffmpeg. The one end-to-end test
 * is skipped when ffmpeg is absent rather than failing — a contributor without
 * it should still get a green suite, and CI has it.
 */

import assert from 'node:assert/strict'
import test, { before } from 'node:test'
import { execFileSync } from 'node:child_process'

let ingest

before(async () => {
  process.env.PMG_DB = ':memory:'
  ingest = await import('./ingest.js')
})

const hasFfmpeg = () => {
  try {
    execFileSync('ffmpeg', ['-version'], { stdio: 'ignore' })
    return true
  } catch {
    return false
  }
}

const info = (over = {}) => ({
  codec: 'h264', width: 1920, height: 1080, fps: 30,
  duration_s: 10, has_alpha: false, pix_fmt: 'yuv420p', audio: 1, ...over,
})

test('all-intra media is stored untouched', () => {
  for (const codec of ['prores', 'dnxhd', 'hap', 'qtrle']) {
    const v = ingest.judge(info({ codec }))
    assert.equal(v.action, 'accept', `${codec} should pass through`)
    // Re-encoding a ProRes master to ProRes would cost a generation of quality
    // and a lot of disk for nothing.
    assert.match(v.reason, /all-intra/)
  }
})

test('long-GOP media is converted, and the reason names the real problem', () => {
  const v = ingest.judge(info({ codec: 'h264' }))
  assert.equal(v.action, 'convert')
  // "Unsupported format" sends someone to a forum. This says what is wrong
  // with THEIR file and what is about to happen to it.
  assert.match(v.reason, /h264/)
  assert.match(v.reason, /seek/)
  assert.match(v.reason, /ProRes 422 HQ/)
})

test('alpha is preserved by choosing 4444 over 422', () => {
  const withAlpha = ingest.judge(info({ codec: 'vp9', has_alpha: true }))
  assert.match(withAlpha.reason, /4444/)
  assert.match(withAlpha.reason, /alpha preserved/)

  // And 4444 is NOT used when there is no alpha: it roughly doubles the file
  // for a channel that does not exist.
  const without = ingest.judge(info({ codec: 'vp9', has_alpha: false }))
  assert.match(without.reason, /422 HQ/)
})

test('an unknown codec is converted rather than refused', () => {
  // Being unable to name it is not a reason to refuse it — ffmpeg may well
  // read it, and the conversion is what makes it playable either way.
  const v = ingest.judge(info({ codec: 'cinepak' }))
  assert.equal(v.action, 'convert')
})

test('a file with no video is refused, and that is different from a bad codec', () => {
  assert.equal(ingest.judge(null).action, 'reject')
  assert.match(ingest.judge(null).reason, /does not contain video/)
  assert.equal(ingest.judge(info({ width: 0, height: 0 })).action, 'reject')
})

test('probing a non-media file returns null rather than throwing', { skip: !hasFfmpeg() }, async () => {
  const fs = await import('node:fs')
  fs.writeFileSync('/tmp/pmg-not-media.txt', 'nope')
  assert.equal(await ingest.probe('/tmp/pmg-not-media.txt'), null)
  fs.rmSync('/tmp/pmg-not-media.txt', { force: true })
})
