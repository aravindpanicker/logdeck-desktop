/**
 * The active filter, and the view derived from it.
 *
 * `filtered` is computed from `items` on every render through `useMemo`. It is
 * never stored, so it cannot drift from the Session Record it describes (§6).
 */

import { useCallback, useMemo, useState } from "react";

import {
  ALL_LEVELS_ACTIVE,
  filterItems,
  levelsAtOrAbove,
  type StreamFilter,
} from "../lib/filter";
import type { Level, StreamItem } from "../lib/types";

export interface UseFilters {
  /** `items` narrowed by the active filter. Derived, never held. */
  readonly filtered: StreamItem[];
  readonly query: string;
  readonly levels: ReadonlySet<Level>;
  /** True when the view is showing less than the whole Session Record. */
  readonly isFiltering: boolean;
  setQuery(query: string): void;
  setLevels(levels: ReadonlySet<Level>): void;
  /** Convenience for the common case: show this **Level** and everything above. */
  setMinLevel(level: Level): void;
}

export function useFilters(items: readonly StreamItem[]): UseFilters {
  const [query, setQuery] = useState("");
  const [levels, setLevels] = useState<ReadonlySet<Level>>(ALL_LEVELS_ACTIVE);

  const filter: StreamFilter = useMemo(() => ({ levels, query }), [levels, query]);
  const filtered = useMemo(() => filterItems(items, filter), [items, filter]);

  const setMinLevel = useCallback((level: Level) => {
    setLevels(levelsAtOrAbove(level));
  }, []);

  return {
    filtered,
    query,
    levels,
    isFiltering: query.trim() !== "" || levels.size !== ALL_LEVELS_ACTIVE.size,
    setQuery,
    setLevels,
    setMinLevel,
  };
}
