/**
 * Renders one string with its search hits marked.
 *
 * The split is done by `lib/highlight.ts`, which guarantees the runs partition
 * the text — so this component can only ever change how the text looks, never
 * what it says.
 */

import { useMemo } from "react";

import { splitHighlights } from "../../lib/highlight";

interface HighlightedProps {
  readonly text: string;
  readonly query: string;
}

export function Highlighted({ text, query }: HighlightedProps) {
  const runs = useMemo(() => splitHighlights(text, query), [text, query]);

  return (
    <>
      {runs.map((run, position) =>
        run.isMatch ? (
          // Index keys are correct here and only here: the runs are a positional
          // partition of one string, so a run has no identity of its own.
          <mark key={position} className="log-hit">
            {run.text}
          </mark>
        ) : (
          run.text
        ),
      )}
    </>
  );
}
