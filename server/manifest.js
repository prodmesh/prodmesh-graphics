/**
 * The manifest sidecar validator — and the edge of what this server knows about
 * the scene format.
 *
 * # Why this file is so small, and must stay so
 *
 * The server stores a SceneDoc as an **opaque CAS blob** and validates only the
 * small JSON manifest the Rust compiler emits alongside it (ADR 0004). It never
 * parses the scene format.
 *
 * The temptation to "just peek at the box tree" from JavaScript is the road to
 * two implementations of the thing that must never disagree — the same
 * discipline that keeps `arrangeSlides()` a single expansion in ProdMesh-Control.
 * A second implementation does not fail loudly; it drifts, and the drift shows
 * up as a graphic that looks right in Design and wrong on air.
 *
 * So: everything here is about *shape*, and the shape is the compiler's output
 * contract (`pmg_scene::validate::Manifest`). If a check here needs to know what
 * a `kind` means, it belongs in Rust.
 *
 * Spec: `docs/specs/scene-format.md` §8.
 */

const HASH_RE = /^blake3:[0-9a-f]{64}$/
const FIELD_TYPES = new Set(['text', 'number', 'bool', 'image', 'video', 'color', 'enum', 'timer'])

/** The scene-format major this server's node fleet is expected to read. */
export const SCENE_READER_MAJOR = 1

class Invalid extends Error {
  constructor(detail) {
    super(detail)
    this.code = 'validation_failed'
    this.detail = detail
  }
}

const str = (v) => typeof v === 'string' && v.length > 0
const int = (v) => Number.isInteger(v)

/**
 * Validate a manifest's shape. Returns it normalised; throws `Invalid`.
 *
 * @param {object} m
 * @param {string} blobHash The address the SceneDoc actually stored under.
 */
export function validateManifest(m, blobHash) {
  if (!m || typeof m !== 'object') throw new Invalid('manifest must be an object')

  for (const k of ['template_rev', 'template_id', 'name']) {
    if (!str(m[k])) throw new Invalid(`manifest.${k} must be a non-empty string`)
  }
  if (!HASH_RE.test(m.template_rev)) {
    throw new Invalid('manifest.template_rev must be blake3:<64 hex>')
  }

  // The one cross-check that matters, and the reason this function takes the
  // blob hash at all: `template_rev` is BY DEFINITION blake3(canonical CBOR of
  // the SceneDoc). If the claimed revision is not the address the bytes landed
  // at, the two halves of this publish do not belong together — a stale
  // manifest paired with fresh bytes, or the reverse. Storing that pair would
  // create a revision whose id is a lie, permanently, because revisions are
  // immutable.
  if (blobHash && m.template_rev !== blobHash) {
    throw new Invalid(
      `template_rev ${m.template_rev} does not match the SceneDoc's address ${blobHash}`,
    )
  }

  const fv = m.format_version
  if (!fv || !int(fv.major) || !int(fv.minor)) {
    throw new Invalid('manifest.format_version must be {major, minor} integers')
  }
  if (!int(m.min_reader) || m.min_reader < 1) {
    throw new Invalid('manifest.min_reader must be a positive integer')
  }

  if (!Array.isArray(m.fields)) throw new Invalid('manifest.fields must be an array')
  const seen = new Set()
  for (const f of m.fields) {
    if (!str(f?.key)) throw new Invalid('every field needs a key')
    if (seen.has(f.key)) throw new Invalid(`duplicate field key ${f.key}`)
    seen.add(f.key)
    if (!FIELD_TYPES.has(f.type)) throw new Invalid(`field ${f.key} has unknown type ${f.type}`)
    if (f.max_len != null && !int(f.max_len)) throw new Invalid(`field ${f.key} max_len must be an integer`)
  }

  if (!Array.isArray(m.timelines) || m.timelines.some((t) => !str(t))) {
    throw new Invalid('manifest.timelines must be an array of names')
  }
  if (!Array.isArray(m.assets) || m.assets.some((a) => !HASH_RE.test(a))) {
    throw new Invalid('manifest.assets must be an array of content addresses')
  }
  if (m.warnings != null && !Array.isArray(m.warnings)) {
    throw new Invalid('manifest.warnings must be an array')
  }
  // ADR 0021: the theme this revision's token values were resolved under —
  // a content address like everything else, when it is there at all.
  if (m.theme != null && !HASH_RE.test(m.theme)) {
    throw new Invalid('manifest.theme must be a content address')
  }

  return m
}

/**
 * Validate an Item's data against its pinned revision's field schema.
 *
 * This runs at **save** time, not at take time. A rundown that will fail should
 * fail while someone is looking at it on Thursday, not at 10:15 on Sunday — the
 * same instinct behind refusing an unreadable template at assignment rather
 * than at take (ADR 0005 §9.1).
 *
 * @returns {string[]} human-readable problems, empty when the data is good.
 */
/** The closed sets `pmg_scene::timer` defines. Restated here because the server
 *  validates Item data and never parses a SceneDoc — and kept short enough that
 *  a mismatch is obvious in review. */
const TIMER_KINDS = ['clock', 'countdown', 'countup', 'elapsed']
const TIMER_FORMATS = ['hh_mm_ss', 'h_mm_ss', 'mm_ss', 'm_ss', 'hh_mm', 'h_mm', 'h_mm_am_pm']

export function validateItemData(manifest, data, { hasAsset = () => true, alsoKnown = null } = {}) {
  const problems = []
  if (!data || typeof data !== 'object' || Array.isArray(data)) {
    return ['Item data must be an object of field values.']
  }

  const byKey = new Map((manifest.fields ?? []).map((f) => [f.key, f]))

  for (const f of manifest.fields ?? []) {
    const v = data[f.key]
    const missing = v == null || v === ''

    if (f.required && missing) {
      problems.push(`${f.label ?? f.key} is required.`)
      continue
    }
    if (missing) continue

    switch (f.type) {
      case 'text':
        if (typeof v !== 'string') problems.push(`${f.label ?? f.key} must be text.`)
        else if (f.max_len != null && [...v].length > f.max_len) {
          // Count code points, not UTF-16 units: `max_len` is about how much
          // will fit in a box, and an emoji is one glyph but two units.
          problems.push(`${f.label ?? f.key} is longer than ${f.max_len} characters.`)
        }
        break
      case 'number':
        if (typeof v !== 'number' || !Number.isFinite(v)) problems.push(`${f.label ?? f.key} must be a number.`)
        break
      case 'bool':
        if (typeof v !== 'boolean') problems.push(`${f.label ?? f.key} must be true or false.`)
        break
      case 'color':
        if (typeof v !== 'string' || !/^#[0-9a-fA-F]{6}([0-9a-fA-F]{2})?$/.test(v)) {
          problems.push(`${f.label ?? f.key} must be a colour like #RRGGBB.`)
        }
        break
      case 'enum':
        if (!Array.isArray(f.options) || !f.options.includes(v)) {
          problems.push(`${f.label ?? f.key} must be one of: ${(f.options ?? []).join(', ')}.`)
        }
        break
      case 'timer': {
        // A timer value is a DECLARATION, not a string: the node computes the
        // number from its own clock (ADR 0022). Validated closely here because
        // this is the last place a person is looking at it — a malformed
        // countdown discovered at 10:15 on Sunday is a countdown nobody can
        // fix.
        const t = v && typeof v === 'object' && !Array.isArray(v) ? v.timer : null
        if (!t || typeof t !== 'object') {
          problems.push(`${f.label ?? f.key} must be a timer.`)
          break
        }
        if (!TIMER_KINDS.includes(t.kind)) {
          problems.push(`${f.label ?? f.key}: “${t.kind}” is not a kind of timer.`)
        }
        if (!TIMER_FORMATS.includes(t.format)) {
          problems.push(`${f.label ?? f.key}: “${t.format}” is not a timer format.`)
        }
        // `at` is what a countdown counts to; without one it counts to now and
        // sits at zero, which looks exactly like a broken graphic.
        if ((t.kind === 'countdown' || t.kind === 'countup') && !Number.isFinite(t.at)) {
          problems.push(`${f.label ?? f.key} needs a time to count ${t.kind === 'countdown' ? 'to' : 'from'}.`)
        }
        break
      }
      case 'image':
      case 'video':
        if (!HASH_RE.test(String(v))) {
          problems.push(`${f.label ?? f.key} must be a content address.`)
        } else if (!hasAsset(v)) {
          // Resolvable NOW, on the server. Whether the *node* has it yet is a
          // separate question answered by item readiness — an Item is valid
          // before it is armable.
          problems.push(`${f.label ?? f.key} refers to an asset this server does not have.`)
        }
        break
    }
  }

  // Unknown keys are refused rather than dropped. Silently discarding a value
  // an operator typed is how a graphic goes on air missing a line with nobody
  // able to say why — and the usual cause is a typo the message can name.
  //
  // `alsoKnown` is what makes ADR 0018 actually work. One Item goes to several
  // destinations, each rendering it with ITS OWN template, and those templates
  // legitimately want different fields: a lower third showing only the lyric, a
  // wall showing the lyric and the song title. A key this template does not
  // declare is not a typo when a SIBLING destination declares it — it is the
  // other destination's field, and this one simply does not use it. Only a key
  // no destination knows at all is the typo this check exists for.
  for (const k of Object.keys(data)) {
    if (byKey.has(k)) continue
    if (alsoKnown?.has(k)) continue
    problems.push(`This template has no field called “${k}”.`)
  }

  return problems
}
