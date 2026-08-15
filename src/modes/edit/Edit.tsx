/**
 * Edit — for the person preparing this week's service.
 *
 * Items (data bound to a Template plus a destination), Playlists, Rundown
 * order, and readiness.
 *
 * Readiness is not advisory here: an Item whose node has not finished staging
 * its assets is *not armable*, and this mode is where that is visible in time
 * to fix it (ADR 0006).
 */
export function Edit() {
  return (
    <>
      <h1>Edit</h1>
      <div className="mode__stub">
        <p>Items, Playlists, and the Rundown.</p>
        <p>
          Not built yet — Items land in <code>M1</code>, Playlists and the
          Rundown in <code>M2</code>. See <code>docs/ROADMAP.md</code>.
        </p>
      </div>
    </>
  )
}
