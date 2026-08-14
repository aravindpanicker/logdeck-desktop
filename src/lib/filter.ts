/**
 * Pure predicates over the **Session Record**.
 *
 * Filtering is a derivation, never a second buffer — `useFilters` calls
 * `filterItems` inside a `useMemo` and keeps no copy of its own, because a
 * filtered array that drifts from `items` is the most likely way this view
 * corrupts itself (§6).
 */

import {
  isBreak,
  LEVELS,
  levelRank,
  type Level,
  type StreamItem,
} from "./types";

export interface StreamFilter {
  /**
   * The **Level**s currently shown. An empty set shows no Entries — the set is
   * the literal state of the toolbar's toggles, not a "no filter" sentinel.
   */
  readonly levels: ReadonlySet<Level>;
  /** Free text; blank means every Entry passes. */
  readonly query: string;
}

/** The default: nothing is filtered out. */
export const ALL_LEVELS_ACTIVE: ReadonlySet<Level> = new Set(LEVELS);

/**
 * The Levels at or above `min` in **severity** order.
 *
 * Severity comes from `levelRank`, not from string comparison: `alert` and
 * `critical` sort before `error` alphabetically but rank above it, so a naive
 * comparison would hide the most serious Entries in the app.
 */
export function levelsAtOrAbove(min: Level): ReadonlySet<Level> {
  const threshold = levelRank(min);
  return new Set(LEVELS.filter((level) => levelRank(level) >= threshold));
}

/**
 * Whether the query occurs anywhere in the Entry (D7).
 *
 * The haystack is `raw` — header, JSON context, and every stack frame — so a hit
 * inside collapsed context is still found and can be revealed.
 */
export function matchesQuery(raw: string, query: string): boolean {
  const needle = query.trim().toLowerCase();
  if (needle === "") return true;
  return raw.toLowerCase().includes(needle);
}

/**
 * `raw` case-folded once per **Entry** rather than once per keystroke.
 *
 * `filterItems` runs over the whole capped buffer on every recompute, and an
 * Entry's `raw` holds its entire stack trace, so re-lowercasing it each time
 * allocates a multi-kilobyte string per Entry per pass. Keyed on the item
 * object, which the buffer replaces whenever an Entry is revised (D2), so a
 * stale fold cannot outlive the text it was taken from; the map is weak, so a
 * trimmed Entry's fold is collected with it.
 */
const folds = new WeakMap<object, string>();

function foldedItemRaw(item: object, raw: string): string {
  const held = folds.get(item);
  if (held !== undefined) return held;
  const fold = raw.toLowerCase();
  folds.set(item, fold);
  return fold;
}

export function matchesFilter(item: StreamItem, filter: StreamFilter): boolean {
  // A **Break** carries no Level and no text: it is the structure of the
  // Session Record, and hiding it would misrepresent what is either side of it.
  if (isBreak(item)) return true;
  if (!filter.levels.has(item.level)) return false;
  const needle = filter.query.trim().toLowerCase();
  if (needle === "") return true;
  return foldedItemRaw(item, item.raw).includes(needle);
}

/** A new array; the source is never touched. */
export function filterItems(
  items: readonly StreamItem[],
  filter: StreamFilter,
): StreamItem[] {
  return items.filter((item) => matchesFilter(item, filter));
}
