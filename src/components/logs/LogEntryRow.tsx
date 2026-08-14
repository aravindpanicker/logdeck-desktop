/**
 * One **Entry** — one row, always.
 *
 * A 47-frame trace is one logical event, so it is one row with its context
 * collapsed behind a disclosure; if a trace were allowed to render open it would
 * bury the Entry after it and the stream would stop being scannable (§6).
 *
 * Two consequences follow and both are handled here:
 *
 * - Copy sends `entry.raw` — header, JSON context, and every frame — because the
 *   collapsed frames are still part of the event (D1).
 * - Search reads `raw` too, so an Entry can match on text the reader cannot see.
 *   Such an Entry reveals itself: it expands, says how many hits are down there,
 *   and marks them (D7). Skipping that would make searching a trace pointless.
 *
 * The row is memoised and keyed by `EntryId`. The buffer revises an Entry in
 * place as its continuation lines arrive (D2), which replaces exactly one object
 * in the snapshot — so with live appends landing every 300 ms only the revised
 * row re-renders, not the 300 around it.
 */

import { memo, useMemo, useState } from "react";

import { countMatches } from "../../lib/highlight";
import type { LogEntry } from "../../lib/types";
import { Highlighted } from "./Highlighted";

interface LogEntryRowProps {
  readonly entry: LogEntry;
  /** The live search text, verbatim from the toolbar. */
  readonly query: string;
  /** True for the moment after this Entry was put on the clipboard. */
  readonly isCopied: boolean;
  readonly onCopy: (entry: LogEntry) => void;
}

/** `[stacktrace]`, the JSON context, and the frames are all just lines. */
function countLines(context: string): number {
  if (context === "") return 0;
  let lines = 1;
  for (let position = 0; position < context.length; position += 1) {
    if (context[position] === "\n") lines += 1;
  }
  return lines;
}

/**
 * The timestamp is stored verbatim and comes in three shapes — bare, with
 * microseconds, and with an offset (§4). The gutter shows the wall-clock time
 * only; the full string stays available on hover, unmodified.
 */
function readClock(timestamp: string): string {
  const trimmed = timestamp.trim();
  if (trimmed === "") return "--:--:--";
  const separator = trimmed.indexOf(" ") >= 0 ? " " : "T";
  const cut = trimmed.indexOf(separator);
  const time = cut >= 0 ? trimmed.slice(cut + 1) : trimmed;
  return time.slice(0, 8);
}

export const LogEntryRow = memo(function LogEntryRow({
  entry,
  query,
  isCopied,
  onCopy,
}: LogEntryRowProps) {
  const hasContext = entry.context !== "";

  // Counted rather than split: the runs of a long trace are only worth
  // allocating once the disclosure is actually open.
  const contextMatches = useMemo(
    () => (hasContext ? countMatches(entry.context, query) : 0),
    [hasContext, entry.context, query],
  );

  // `null` means "nobody has decided" — the row follows the search. A click is a
  // decision and holds until *this row's* match state changes, so the reveal in
  // D7 is not permanently disabled by one collapse.
  //
  // Deliberately keyed on `contextMatches`, not on `query`: a row the reader
  // opened by hand must not slam shut because they started typing something
  // that has nothing to do with it. Only a row whose own hit count moved has
  // been given new information, and only that row forgets the old decision.
  const [choice, setChoice] = useState<boolean | null>(null);
  const [lastMatches, setLastMatches] = useState(contextMatches);
  if (lastMatches !== contextMatches) {
    setLastMatches(contextMatches);
    setChoice(null);
  }

  const isExpanded = choice ?? contextMatches > 0;
  const lines = useMemo(() => countLines(entry.context), [entry.context]);

  const matchNote =
    contextMatches === 1 ? "1 match" : `${contextMatches} matches`;

  return (
    <li
      className="log-row"
      data-item-id={entry.id}
      data-level={entry.level}
      data-expanded={isExpanded ? "true" : undefined}
      data-hit={contextMatches > 0 ? "true" : undefined}
      // Reachable by keyboard: the reader tabs down the stream and the row's
      // controls come into reach as it takes focus.
      tabIndex={0}
    >
      <span className="log-row__rail" aria-hidden="true" />

      {/*
        Not a `<time>`: the timestamp is stored verbatim, and Laravel's format is
        not a valid HTML `datetime` value. Asserting machine-readability we do
        not have would be worse than presenting it as the text it is.
      */}
      <span className="log-row__time" title={entry.timestamp}>
        {readClock(entry.timestamp)}
      </span>

      <span className="log-row__level" title={`${entry.env}.${entry.level}`}>
        {entry.level}
      </span>

      <p className="log-row__message">
        <Highlighted text={entry.message} query={query} />
      </p>

      <button
        type="button"
        className="log-row__copy"
        data-copied={isCopied ? "true" : undefined}
        onClick={() => onCopy(entry)}
        aria-label={`Copy the whole ${entry.level} Entry from ${
          entry.timestamp || "an unknown time"
        }, including its context and every stack frame`}
      >
        {isCopied ? "Copied" : "Copy"}
      </button>

      {hasContext && (
        <div className="log-row__context">
          <button
            type="button"
            className="log-row__disclosure"
            onClick={() => setChoice(!isExpanded)}
            aria-expanded={isExpanded}
            aria-controls={`context-${entry.id}`}
          >
            <span className="log-row__caret" aria-hidden="true">
              {isExpanded ? "▾" : "▸"}
            </span>
            <span className="log-row__disclosure-label">
              context, {lines} {lines === 1 ? "line" : "lines"}
            </span>
            {contextMatches > 0 && (
              <span className="log-row__match-count">{matchNote}</span>
            )}
          </button>

          {isExpanded && (
            <pre className="log-row__frames" id={`context-${entry.id}`}>
              <Highlighted text={entry.context} query={query} />
            </pre>
          )}
        </div>
      )}
    </li>
  );
});
