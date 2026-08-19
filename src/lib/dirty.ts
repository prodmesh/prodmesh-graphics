/**
 * Unsaved-work guard, app-wide.
 *
 * A mode with unpublished changes registers a sentence here; navigation asks
 * before proceeding. Module state rather than context, deliberately: the
 * AppShell's tabs and a mode's own internal navigation both need the same
 * answer, and threading a provider through every candidate leaver is more
 * machinery than one flag warrants.
 *
 * The browser-level half (`beforeunload`) rides the same flag, so closing the
 * tab gets the native prompt while the flag is set. The in-app half is
 * `window.confirm` — synchronous on purpose, because a NavLink click needs its
 * answer before the default is or is not prevented, and a themed async dialog
 * cannot give one.
 */

let reason: string | null = null

const onBeforeUnload = (e: BeforeUnloadEvent) => {
  // The browser shows its own wording; this only opts in.
  e.preventDefault()
}

/** Register (or clear, with null) the sentence shown before leaving. */
export function setDirty(next: string | null) {
  const had = reason !== null
  reason = next
  if (next !== null && !had) window.addEventListener('beforeunload', onBeforeUnload)
  if (next === null && had) window.removeEventListener('beforeunload', onBeforeUnload)
}

/**
 * True when leaving is fine — either nothing is dirty, or the user just said
 * to go anyway. Does NOT clear the flag: the leaving surface clears its own
 * dirty state when it actually unloads, so a cancelled navigation costs
 * nothing.
 */
export function confirmLeave(): boolean {
  if (reason === null) return true
  return window.confirm(reason)
}

/** Test seam. Nothing in production may call it. */
export function _reset() {
  setDirty(null)
}
