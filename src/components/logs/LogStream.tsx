/**
 * The **Session Record** on screen: **Entries** and **Breaks**, in order.
 *
 * Everything here exists to keep a live view readable while a 5 MB file is being
 * appended to:
 *
 * - **The render window is the most recent 300 matches** (§6). Recency-first is
 *   how logs are read, so a virtualiser is not earned; a bounded list is.
 * - **Rows are memoised and keyed by id.** A revision (D2) replaces exactly one
 *   object in the snapshot, so one row re-renders, not three hundred.
 * - **Auto-scroll is a lease, not a law.** It holds while the reader is at the
 *   bottom and releases the moment they scroll up. Yanking someone back down
 *   mid-trace is the fastest way to make a live tail unusable.
 * - **While the lease is released, the window's START is held.** The window
 *   normally slides, dropping a row off the top per arrival — which shortens the
 *   content above a scrolled-up reader and carries them downward without
 *   anything ever assigning `scrollTop`. See `streamWindow.ts`.
 * - **Paging earlier keeps the reader's place.** The first rendered item is
 *   measured before the page lands and the scroll position is corrected by how
 *   far it moved, so nothing jumps under the eye. See `scrollAnchor.ts`.
 */

import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";

import { useFilters } from "../../hooks/useFilters";
import { useLogStream } from "../../hooks/useLogStream";
import { isBreak, isEntry, type ProjectId } from "../../lib/types";
import { BreakMarker } from "./BreakMarker";
import { LogEntryRow } from "./LogEntryRow";
import { LogToolbar } from "./LogToolbar";
import { useCopyEntry } from "./useCopyEntry";
import { anchorCorrection, type ScrollAnchor } from "./scrollAnchor";
import { windowStart } from "./streamWindow";
import "./log-stream.css";

/** The most recent N matches are rendered; earlier ones are a click away (§6). */
const RENDER_WINDOW = 300;

/**
 * How close to the bottom still counts as "at the bottom". A row is taller than
 * a line, and sub-pixel scroll heights mean an exact comparison flickers.
 */
const PIN_SLACK_PX = 32;

interface LogStreamProps {
  readonly projectId: ProjectId;
}

export function LogStream({ projectId }: LogStreamProps) {
  const {
    items,
    isLoading,
    isLoadingEarlier,
    error,
    target,
    isRetargeting,
    loadEarlier,
    setTarget,
  } = useLogStream(projectId);
  const { filtered, query, levels, isFiltering, setQuery, setLevels } =
    useFilters(items);
  const { copiedId, notice, copy } = useCopyEntry();

  const scrollerRef = useRef<HTMLDivElement>(null);
  const anchorRef = useRef<ScrollAnchor | null>(null);
  /** Read inside the layout effect, where state would be a render behind. */
  const isPinnedRef = useRef(true);
  const [isPinned, setIsPinned] = useState(true);
  const [windowSize, setWindowSize] = useState(RENDER_WINDOW);
  /**
   * Match count at the moment the reader scrolled up, or `null` while pinned.
   * Holding the window's start against this is what stops an arriving Entry
   * from dropping a row above the viewport and dragging the reader down.
   */
  const [heldAt, setHeldAt] = useState<number | null>(null);
  const filteredCountRef = useRef(0);
  filteredCountRef.current = filtered.length;

  // A different filter is a different set of matches; the window starts again at
  // the most recent 300 of them rather than keeping a size the reader grew for
  // some earlier query.
  useEffect(() => {
    setWindowSize(RENDER_WINDOW);
    setHeldAt(isPinnedRef.current ? null : filteredCountRef.current);
  }, [query, levels]);

  const windowed = useMemo(
    () => filtered.slice(windowStart(filtered.length, windowSize, heldAt)),
    [filtered, windowSize, heldAt],
  );

  // Counts are of **Entries**. A Break is the structure of the Session Record
  // rather than something in it, and counting it would overstate what is held.
  const counts = useMemo(() => {
    let total = 0;
    for (const item of items) if (isEntry(item)) total += 1;
    let shown = 0;
    for (const item of filtered) if (isEntry(item)) shown += 1;
    return { total, shown };
  }, [items, filtered]);

  const hasEarlier = filtered.length > windowed.length;

  const handleScroll = useCallback((): void => {
    const scroller = scrollerRef.current;
    if (scroller === null) return;
    const distance =
      scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight;
    const next = distance <= PIN_SLACK_PX;
    if (next === isPinnedRef.current) return;
    isPinnedRef.current = next;
    setIsPinned(next);
    setHeldAt(next ? null : filteredCountRef.current);
  }, []);

  const findRow = (scroller: HTMLElement, id: string): HTMLElement | null =>
    scroller.querySelector<HTMLElement>(`[data-item-id="${CSS.escape(id)}"]`);

  /**
   * Layout, not effect: the correction has to be applied in the same frame the
   * new rows are laid out in, or the reader sees the jump before it is undone.
   */
  useLayoutEffect(() => {
    const scroller = scrollerRef.current;
    if (scroller === null) return;

    const anchor = anchorRef.current;
    if (anchor !== null) {
      anchorRef.current = null;
      const row = findRow(scroller, anchor.id);
      // The arithmetic is `scrollAnchor.ts`; the measuring is this effect's.
      const correction = anchorCorrection(anchor, row === null ? null : row.offsetTop);
      if (correction.kind === "correct") {
        scroller.scrollTop = correction.scrollTop;
        return;
      }
      // The anchored row is still where it was — leaving the scroller alone is
      // the correction. Only a lost anchor falls through to the lease below.
      if (correction.kind === "unchanged") {
        return;
      }
    }

    // Only when the lease is held. Setting scrollTop while the reader is up in
    // the history is exactly the "fighting them" this must never do.
    if (isPinnedRef.current) {
      scroller.scrollTop = scroller.scrollHeight;
    }
  }, [windowed]);

  const rememberAnchor = useCallback((): void => {
    const scroller = scrollerRef.current;
    const first = windowed[0];
    if (scroller === null || first === undefined) return;
    const row = findRow(scroller, first.id);
    if (row === null) return;
    anchorRef.current = {
      id: first.id,
      top: row.offsetTop,
      scrollTop: scroller.scrollTop,
    };
  }, [windowed]);

  const handleLoadEarlier = useCallback((): void => {
    rememberAnchor();
    // Widen first either way: a page fetched from disk lands *above* what is
    // held, so without this the window would still show the newest 300 and the
    // page the reader asked for would be invisible.
    setWindowSize((size) => size + RENDER_WINDOW);
    if (hasEarlier) return;

    void loadEarlier().then(() => {
      // If nothing arrived, no layout pass consumed the anchor. Drop it after
      // paint so a later append is not measured against a stale position.
      requestAnimationFrame(() => {
        anchorRef.current = null;
      });
    });
  }, [hasEarlier, loadEarlier, rememberAnchor]);

  const jumpToLatest = useCallback((): void => {
    const scroller = scrollerRef.current;
    if (scroller === null) return;
    isPinnedRef.current = true;
    setIsPinned(true);
    // Releasing the hold lets the window slide again; without this it would stay
    // frozen at wherever the reader scrolled up and grow without bound.
    setHeldAt(null);
    scroller.scrollTop = scroller.scrollHeight;
  }, []);

  const isEmpty = windowed.length === 0;

  return (
    <div className="log-stream">
      <LogToolbar
        projectId={projectId}
        target={target}
        isRetargeting={isRetargeting}
        setTarget={setTarget}
        query={query}
        levels={levels}
        isFiltering={isFiltering}
        shown={counts.shown}
        total={counts.total}
        setQuery={setQuery}
        setLevels={setLevels}
      />

      {error !== null && (
        <p className="log-stream__error" role="alert">
          {error}
        </p>
      )}

      <div
        className="log-stream__scroller"
        ref={scrollerRef}
        onScroll={handleScroll}
        tabIndex={-1}
      >
        <div className="log-stream__earlier">
          <button
            type="button"
            className="log-stream__earlier-button"
            onClick={handleLoadEarlier}
            disabled={isLoadingEarlier || isLoading}
          >
            {isLoadingEarlier ? "Reading earlier Entries…" : "Load earlier"}
          </button>
          <span className="log-stream__earlier-note">
            {hasEarlier
              ? `${filtered.length - windowed.length} older matches held`
              : "Pages another window in from the file"}
          </span>
        </div>

        {isEmpty ? (
          <p className="log-stream__vacant">
            {isLoading
              ? "Reading the opening window…"
              : counts.total === 0
                ? "Nothing has been written to this Project yet. Entries appear here as they land."
                : "No Entry matches the current filter. The Session Record is intact — only this view is narrowed."}
          </p>
        ) : (
          <ul className="log-stream__items">
            {windowed.map((item) =>
              isBreak(item) ? (
                <BreakMarker key={item.id} item={item} />
              ) : (
                <LogEntryRow
                  key={item.id}
                  entry={item}
                  query={query}
                  isCopied={copiedId === item.id}
                  onCopy={copy}
                />
              ),
            )}
          </ul>
        )}
      </div>

      <div className="log-stream__foot">
        <p className="log-stream__announce" role="status" aria-live="polite">
          {notice}
        </p>
        {!isPinned && (
          <button
            type="button"
            className="log-stream__jump"
            onClick={jumpToLatest}
          >
            Jump to latest
          </button>
        )}
        <p className="log-stream__tail" data-live={isPinned ? "true" : undefined}>
          {isPinned ? "Following" : "Held"}
        </p>
      </div>
    </div>
  );
}
