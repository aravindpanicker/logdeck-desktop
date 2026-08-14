/**
 * The **Target** picker, the **Level** filter and the search box.
 *
 * The Target control comes first because it names what is being read; the
 * filter and the search narrow it. See `TargetPicker` — a pinned Target still
 * streams.
 *
 * The Level control is nine toggles rather than a minimum-severity slider,
 * because the question a developer actually asks is "show me only the errors",
 * not "show me everything at least this bad". Each toggle wears its own rail, so
 * the filter is read with the same visual language as the gutter beside it.
 *
 * `unknown` sits apart, after a divider: it is the absence of a severity rather
 * than a low one, and grouping it with the real Levels would imply the file said
 * something it never said.
 */

import { memo, useCallback, useState, useTransition } from "react";

import { ALL_LEVELS_ACTIVE } from "../../lib/filter";
import {
  LEVELS,
  type Level,
  type ProjectId,
  type Target,
} from "../../lib/types";
import { TargetPicker } from "./TargetPicker";

interface LogToolbarProps {
  readonly projectId: ProjectId;
  /** Which file the stream is reading — `Latest`, or a pinned one (D5). */
  readonly target: Target;
  readonly isRetargeting: boolean;
  readonly setTarget: (target: Target) => Promise<void>;
  readonly query: string;
  readonly levels: ReadonlySet<Level>;
  readonly isFiltering: boolean;
  /** Entries passing the filter, and the size of the whole Session Record. */
  readonly shown: number;
  readonly total: number;
  readonly setQuery: (query: string) => void;
  readonly setLevels: (levels: ReadonlySet<Level>) => void;
}

/** Most severe first: the reader scans down from the thing they came to find. */
const ORDERED_LEVELS: readonly Level[] = LEVELS.filter(
  (level) => level !== "unknown",
).reverse();

export const LogToolbar = memo(function LogToolbar({
  projectId,
  target,
  isRetargeting,
  setTarget,
  query,
  levels,
  isFiltering,
  shown,
  total,
  setQuery,
  setLevels,
}: LogToolbarProps) {
  /*
   * The box shows what was typed; the filter follows behind.
   *
   * `matchesQuery` scans `raw` — traces included — across the whole capped
   * buffer, so committing a keystroke straight to the filter would run megabytes
   * of case-folded scanning before the character it produced is painted. The
   * draft is urgent state and paints immediately; the filter is a transition and
   * yields to the next keystroke, so typing stays at input speed no matter how
   * much is held.
   */
  const [draft, setDraft] = useState(query);
  const [, startFilter] = useTransition();

  // The filter can also change from outside — "Clear filter" — and when it does
  // it is the truth, so the draft follows it back.
  const [lastQuery, setLastQuery] = useState(query);
  if (lastQuery !== query) {
    setLastQuery(query);
    if (query !== draft) setDraft(query);
  }

  const type = useCallback(
    (value: string): void => {
      setDraft(value);
      startFilter(() => {
        setQuery(value);
      });
    },
    [setQuery],
  );

  const toggle = useCallback(
    (level: Level): void => {
      const next = new Set(levels);
      if (next.has(level)) {
        next.delete(level);
      } else {
        next.add(level);
      }
      setLevels(next);
    },
    [levels, setLevels],
  );

  const reset = useCallback((): void => {
    setQuery("");
    setLevels(ALL_LEVELS_ACTIVE);
  }, [setQuery, setLevels]);

  const renderToggle = (level: Level) => {
    const isOn = levels.has(level);
    return (
      <li key={level}>
        <button
          type="button"
          className="log-toolbar__level"
          data-level={level}
          data-on={isOn ? "true" : undefined}
          aria-pressed={isOn}
          onClick={() => toggle(level)}
        >
          {level}
        </button>
      </li>
    );
  };

  return (
    <div className="log-toolbar">
      <TargetPicker
        projectId={projectId}
        target={target}
        isRetargeting={isRetargeting}
        setTarget={setTarget}
      />

      <ul className="log-toolbar__levels" aria-label="Filter by Level">
        {ORDERED_LEVELS.map(renderToggle)}
        <li className="log-toolbar__divider" aria-hidden="true" />
        {renderToggle("unknown")}
      </ul>

      <div className="log-toolbar__search">
        <label className="log-stream-sr-only" htmlFor="log-search">
          Search Entries
        </label>
        <input
          id="log-search"
          type="search"
          className="log-toolbar__input"
          value={draft}
          spellCheck={false}
          autoComplete="off"
          placeholder="Search the whole Entry, traces included"
          onChange={(event) => type(event.target.value)}
        />
      </div>

      <p className="log-toolbar__count" aria-live="polite">
        <span className="log-toolbar__count-shown">{shown}</span>
        <span className="log-toolbar__count-total">/ {total}</span>
      </p>

      {isFiltering && (
        <button type="button" className="log-toolbar__reset" onClick={reset}>
          Clear filter
        </button>
      )}
    </div>
  );
});
