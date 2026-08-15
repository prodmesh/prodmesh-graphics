/**
 * Configure — for whoever set up the room.
 *
 * Node discovery and adoption, outputs, formats, colour, key mode, layer
 * stacks, reference status, test patterns, network and API settings, users.
 *
 * The test patterns are the point of this mode, not decoration: they are the
 * only way an operator can *prove* a configuration rather than trust it
 * (docs/specs/output-config.md §8).
 */
export function Configure() {
  return (
    <>
      <h1>Configure</h1>
      <div className="mode__stub">
        <p>
          Playout nodes, outputs, formats, colour, key and fill, layer stacks,
          reference, and test patterns.
        </p>
        <p>
          Not built yet — node discovery and adoption land in <code>M2</code>.
          See <code>docs/ROADMAP.md</code>.
        </p>
      </div>
    </>
  )
}
