/**
 * Show — for the person running it.
 *
 * Preview and program, arm, take, take out, clear, the take log, node health.
 *
 * This mode must be BORING. Large targets, no destructive action within reach
 * of a mis-tap, no route into Design, and state in the present tense —
 * "Lower Third · Pastor Dan", not "Item 4 active".
 *
 * And the take button must never lie: POST /api/cue is synchronous, and a
 * 503 cue_not_acked is rendered as words, not swallowed (ADR 0008).
 */
export function Show() {
  return (
    <>
      <h1>Show</h1>
      <div className="mode__stub">
        <p>Preview and program, layers, take and clear, and the take log.</p>
        <p>
          Not built yet — a single take button lands in <code>M1</code>, layers
          and the take log in <code>M2</code>. See <code>docs/ROADMAP.md</code>.
        </p>
      </div>
    </>
  )
}
