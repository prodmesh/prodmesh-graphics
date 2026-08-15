/**
 * Design — for the person who makes it look right.
 *
 * Template authoring: the box tree, text, shapes, media, fields, timelines,
 * sample-data preview, and publish.
 *
 * The canvas renders with pmg-render compiled to WASM — the same code the
 * playout node runs, which is what makes the preview trustworthy (ADR 0001).
 * Every operation must be reachable without a drag: jsdom has no layout, so a
 * simulated drag certifies nothing, and the button-and-keyboard paths are what
 * the tests can actually cover.
 */
export function Design() {
  return (
    <>
      <h1>Design</h1>
      <div className="mode__stub">
        <p>
          Templates: responsive layout, text, shapes, media, and named
          timelines.
        </p>
        <p>
          Not built yet — the canvas needs the WASM renderer, which is gated on
          spikes <code>S1</code> and <code>S6</code>. See{' '}
          <code>docs/ROADMAP.md</code> M0 and M1.
        </p>
      </div>
    </>
  )
}
