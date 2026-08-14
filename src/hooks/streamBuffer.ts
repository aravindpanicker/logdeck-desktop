/**
 * The **Session Record** held for the selected **Project**.
 *
 * Three behaviours interact here and each one can silently corrupt the view:
 *
 * 1. `log:entry` **upserts by id** (D2). The watcher emits a trailing Entry as
 *    soon as its header lands and re-emits it under the same id as continuation
 *    lines arrive, so a revision must replace in place and keep its position — a
 *    47-frame trace arriving late must not push its Entry to the bottom.
 * 2. `log:break` **appends** and removes nothing above it (D3, ADR 0001). The
 *    view is a record of the debugging session, not a mirror of the file.
 * 3. The buffer **caps at 2000 by trimming the front**, and the index must stay
 *    consistent across that trim — a stale position is exactly how an upsert
 *    lands on the wrong row.
 *
 * The index therefore stores an *absolute* position and a separate `origin`
 * holds the absolute position of `items[0]` — it rises by the number trimmed and
 * falls by the number prepended, so after a `load_earlier` page it can be
 * negative. Trimming then costs only the deletions, instead of renumbering every
 * surviving id on every append past the cap.
 *
 * State is mutated inside this closure and never escapes: `snapshot()` hands
 * back a copy, so nothing outside can leave the array and the index disagreeing.
 */

import type { Break, LogEntry, StreamItem } from "../lib/types";

/** ADR 0001 — the retained stream is unbounded in principle, so it is capped. */
export const STREAM_CAP = 2000;

export interface StreamBuffer {
  /** A copy of the retained items, oldest first. */
  snapshot(): StreamItem[];
  size(): number;
  /** Current position of an id, or `undefined` if it is not (or no longer) held. */
  positionOf(id: string): number | undefined;
  /** Replaces the Entry with this id in place, or appends it (D2). */
  upsertEntry(entry: LogEntry): void;
  /** Appends a Break marker; everything above it is retained (D3). */
  appendBreak(brk: Break): void;
  /** Puts an earlier page above what is held, skipping ids already present. */
  prepend(items: readonly StreamItem[]): void;
  /** Drops everything and starts a new Session Record. */
  clear(): void;
}

export function createStreamBuffer(cap: number = STREAM_CAP): StreamBuffer {
  const items: StreamItem[] = [];
  /** id -> absolute position, i.e. position in the stream including trimmed items. */
  const index = new Map<string, number>();
  /** Absolute position of `items[0]`. Rises on trim, falls on prepend. */
  let origin = 0;

  function positionOf(id: string): number | undefined {
    const absolute = index.get(id);
    if (absolute === undefined) return undefined;
    return absolute - origin;
  }

  function append(item: StreamItem): void {
    items.push(item);
    index.set(item.id, origin + items.length - 1);
    trimFront();
  }

  function trimFront(): void {
    const overflow = items.length - cap;
    if (overflow <= 0) return;
    for (let position = 0; position < overflow; position += 1) {
      index.delete(items[position].id);
    }
    items.splice(0, overflow);
    origin += overflow;
  }

  return {
    snapshot: () => [...items],

    size: () => items.length,

    positionOf,

    upsertEntry(entry: LogEntry): void {
      const item: StreamItem = { type: "entry", ...entry };
      const position = positionOf(entry.id);
      if (position !== undefined) {
        items[position] = item;
        return;
      }
      append(item);
    },

    appendBreak(brk: Break): void {
      const item: StreamItem = { type: "break", ...brk };
      if (positionOf(brk.id) !== undefined) return;
      append(item);
    },

    prepend(incoming: readonly StreamItem[]): void {
      const fresh: StreamItem[] = [];
      const seen = new Set<string>();
      for (const item of incoming) {
        if (positionOf(item.id) !== undefined || seen.has(item.id)) continue;
        seen.add(item.id);
        fresh.push(item);
      }
      if (fresh.length === 0) return;

      items.unshift(...fresh);
      origin -= fresh.length;
      for (let position = 0; position < fresh.length; position += 1) {
        index.set(fresh[position].id, origin + position);
      }
      // Deliberately no trim: front-trimming here would discard exactly the page
      // the user just asked for. The cap re-applies as new Entries arrive.
    },

    clear(): void {
      items.length = 0;
      index.clear();
      origin = 0;
    },
  };
}
