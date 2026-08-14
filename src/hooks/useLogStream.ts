/**
 * The live **Session Record** for the selected **Project**.
 *
 * The hook owns a `StreamBuffer` and wires it to three things from the frozen
 * IPC contract (§3): the `select_project` opening window (D6), the `log:entry`,
 * `log:entries` and `log:break` events, and `load_earlier` paging.
 *
 * The Tauri bridge arrives as an injected `LogTransport` so the hook can be
 * driven by a fake that pushes events synchronously; nothing here imports
 * `@tauri-apps/api` directly.
 */

import { useCallback, useEffect, useRef, useState } from "react";

import { createStreamBuffer, type StreamBuffer } from "./streamBuffer";
import {
  describeIpcError,
  tauriTransport,
  type LogTransport,
  type Unlisten,
} from "./transport";
import {
  isEntry,
  type Break,
  type LogEntry,
  type ProjectId,
  type StreamItem,
  type Target,
} from "../lib/types";

const ENTRY_EVENT = "log:entry";
/**
 * A whole poll's closed **Entries** in one event.
 *
 * The watcher can close thousands inside a single 300 ms tick. Each `log:entry`
 * costs a snapshot copy, a full filter pass and a React commit, so an unbatched
 * burst is quadratic in the buffer and freezes the view exactly while it is most
 * worth watching. The batch upserts item by item and publishes once.
 */
const ENTRIES_EVENT = "log:entries";
const BREAK_EVENT = "log:break";

export interface UseLogStream {
  /** The retained items, oldest first. A new array identity on every change. */
  readonly items: StreamItem[];
  /** The opening window is in flight. */
  readonly isLoading: boolean;
  readonly isLoadingEarlier: boolean;
  /** The last failure from the bridge, surfaced rather than swallowed. */
  readonly error: string | null;
  /**
   * What this Project is reading: `"latest"` — the newest file, followed across
   * rotation — or the file the reader has pinned (D5).
   */
  readonly target: Target;
  /** A pin is being applied; the window it returns has not landed yet. */
  readonly isRetargeting: boolean;
  /** Pages another window of Entries in above what is held (D6). */
  loadEarlier(): Promise<void>;
  /**
   * Pins a file, or returns to following the newest one.
   *
   * **Replaces** the Session Record with the window `set_target` returns rather
   * than prepending it. See the call site for why, and ADR 0001 for why that is
   * not a breach of "a Break never clears the view".
   */
  setTarget(target: Target): Promise<void>;
}

/** The default Target, and what a newly selected Project starts on (D5). */
const LATEST: Target = "latest";

const UNKNOWN_FAILURE = "The log stream failed for an unknown reason";

/**
 * A `StreamItem` as it arrives from the bridge, before anything trusts it.
 *
 * `isEntry`/`isBreak` in `types.ts` narrow a value that is *already* a
 * `StreamItem`; nothing coming off the wire is, so the discriminant is read
 * here rather than assumed.
 */
function isStreamItem(value: unknown): value is StreamItem {
  if (typeof value !== "object" || value === null) return false;
  const type = (value as { readonly type?: unknown }).type;
  return type === "entry" || type === "break";
}

/**
 * Narrows a command's reply to a window before it reaches the buffer.
 *
 * `invoke<StreamItem[]>` is a *claim* about the Rust side, not a check on it —
 * the type parameter is erased and the payload is whatever crossed the bridge.
 * The `log:entries` listener already refuses to trust its payload; a command
 * reply gets the same treatment, because `prepend` iterates what it is handed
 * and a non-array throws from inside the buffer with a message naming neither
 * the command nor the shape.
 *
 * A reply that is not an array is a failure, and is raised as one so it lands
 * in `error` where the reader can see it. Individual items that are not Entries
 * or Breaks are dropped rather than admitted: the buffer is keyed by `id` and
 * an item without one would corrupt the upsert for every Entry after it.
 */
function asWindow(command: string, payload: unknown): StreamItem[] {
  if (!Array.isArray(payload)) {
    throw new Error(`${command} did not return a window of log items`);
  }
  return payload.filter(isStreamItem);
}

export function useLogStream(
  projectId: ProjectId | null,
  transport: LogTransport = tauriTransport,
): UseLogStream {
  const bufferRef = useRef<StreamBuffer>(createStreamBuffer());
  /** Mirrors `isLoadingEarlier` synchronously — state lags a re-entrant call. */
  const inFlightRef = useRef(false);
  const [items, setItems] = useState<StreamItem[]>([]);
  const [isLoading, setIsLoading] = useState(false);
  const [isLoadingEarlier, setIsLoadingEarlier] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [target, setTargetState] = useState<Target>(LATEST);
  const [isRetargeting, setIsRetargeting] = useState(false);
  /**
   * Bumped every time the Session Record is replaced wholesale — a new Project,
   * or a new Target. An earlier page still in flight across one of those was
   * paged from a record that no longer exists, and prepending it would put
   * unrelated Entries above the window that replaced it.
   */
  const recordRef = useRef(0);

  useEffect(() => {
    // A new Project is a new Session Record. Selecting a different one is the
    // only thing that discards retained Entries — a Break never does (ADR 0001).
    const buffer = createStreamBuffer();
    bufferRef.current = buffer;
    recordRef.current += 1;
    setItems([]);
    setError(null);
    // A pin belongs to the Project it was made in. The backend agrees — each
    // watcher holds its own Target — so carrying the previous Project's file
    // name across would only mislabel this one.
    setTargetState(LATEST);
    setIsRetargeting(false);

    if (projectId === null) {
      setIsLoading(false);
      return;
    }

    let cancelled = false;
    const unlistens: Unlisten[] = [];
    const publish = () => {
      if (!cancelled) setItems(buffer.snapshot());
    };

    setIsLoading(true);

    void (async () => {
      try {
        // `allSettled`, not `all`: if one subscription rejects, the other may
        // still have registered, and a rejected `all` would lose its Unlisten
        // before it could be recorded — a listener nothing can ever detach.
        const settled = await Promise.allSettled([
          transport.listen<LogEntry>(ENTRY_EVENT, (entry) => {
            if (cancelled || entry.projectId !== projectId) return;
            buffer.upsertEntry(entry);
            publish();
          }),
          transport.listen<LogEntry[]>(ENTRIES_EVENT, (batch) => {
            if (cancelled || !Array.isArray(batch)) return;
            let touched = false;
            for (const entry of batch) {
              if (entry.projectId !== projectId) continue;
              buffer.upsertEntry(entry);
              touched = true;
            }
            // One publish for the whole batch — the point of the event.
            if (touched) publish();
          }),
          transport.listen<Break>(BREAK_EVENT, (brk) => {
            if (cancelled || brk.projectId !== projectId) return;
            buffer.appendBreak(brk);
            publish();
          }),
        ]);

        for (const outcome of settled) {
          if (outcome.status === "fulfilled") unlistens.push(outcome.value);
        }
        // The cleanup may already have run and seen an empty list, so detach here.
        if (cancelled) {
          for (const unlisten of unlistens.splice(0)) unlisten();
          return;
        }
        for (const outcome of settled) {
          if (outcome.status === "rejected") throw outcome.reason;
        }

        // Subscribe first, then fetch the window, and *prepend* it. Everything
        // in the window is older than anything the listeners can have seen, so
        // an Entry that landed while the request was in flight is kept rather
        // than overwritten — and `prepend` skips ids already held.
        const opening = await transport.invoke<unknown>("select_project", {
          projectId,
        });
        if (cancelled) return;
        buffer.prepend(asWindow("select_project", opening));
        publish();
        setError(null);
      } catch (cause) {
        if (!cancelled) setError(describeIpcError(cause, UNKNOWN_FAILURE));
      } finally {
        if (!cancelled) setIsLoading(false);
      }
    })();

    return () => {
      cancelled = true;
      for (const unlisten of unlistens) unlisten();
    };
  }, [projectId, transport]);

  const loadEarlier = useCallback(async () => {
    if (projectId === null) return;
    // A second call in flight would page the same `beforeOffset` again: the
    // buffer dedupes it, but the round trip is wasted either way.
    if (inFlightRef.current) return;

    const buffer = bufferRef.current;
    // The oldest **Entry** — a leading Break carries no offset to page from.
    const oldest = buffer.snapshot().find(isEntry);
    if (oldest === undefined) return;

    const record = recordRef.current;
    inFlightRef.current = true;
    setIsLoadingEarlier(true);
    try {
      // The id, not the offset: it carries the file and the id generation too,
      // and after a **Break** the oldest Entry held belongs to the file *before*
      // the Break rather than the one being tailed now (§3, ADR 0001).
      const earlier = await transport.invoke<unknown>("load_earlier", {
        projectId,
        beforeId: oldest.id,
      });
      // The selection may have changed while this was in flight; that swaps the
      // buffer, and an earlier page for the previous Project must not land in it.
      // A Target change keeps the same buffer but replaces its contents, so the
      // record generation is checked as well.
      if (bufferRef.current !== buffer || recordRef.current !== record) return;
      buffer.prepend(asWindow("load_earlier", earlier));
      setItems(buffer.snapshot());
      // The stream is healthy again; a stale failure must not linger as `error`.
      setError(null);
    } catch (cause) {
      setError(describeIpcError(cause, UNKNOWN_FAILURE));
    } finally {
      inFlightRef.current = false;
      setIsLoadingEarlier(false);
    }
  }, [projectId, transport]);

  const setTarget = useCallback(
    async (next: Target) => {
      if (projectId === null) return;

      const buffer = bufferRef.current;
      setIsRetargeting(true);
      try {
        const opening = await transport.invoke<unknown>("set_target", {
          projectId,
          target: next,
        });
        // The Project may have changed while the pin was in flight; that window
        // belongs to a record nobody is looking at any more.
        if (bufferRef.current !== buffer) return;
        // Narrowed before `clear()`: a reply that is not a window must fail with
        // the record intact rather than empty it and then throw.
        const pinnedWindow = asWindow("set_target", opening);

        /*
         * REPLACE, not prepend — the one place in this hook that discards
         * retained Entries other than a change of Project.
         *
         * The returned window is the *pinned file's* opening window. Prepending
         * it would interleave two unrelated files: pin yesterday's log and its
         * Entries would sit above today's while being older than nothing on
         * screen, so the stream would read backwards in time and `load_earlier`
         * would page from an Entry in the wrong file.
         *
         * ADR 0001 is not breached. That decision governs the source
         * discontinuing *underneath* the reader — a truncation or a rotation
         * they did not ask for — where losing the view costs them the exception
         * they were reading. This is the reader deliberately navigating
         * somewhere else, which is the one case where showing them what they
         * asked for is the whole point.
         */
        buffer.clear();
        buffer.prepend(pinnedWindow);
        recordRef.current += 1;
        setItems(buffer.snapshot());
        setTargetState(next);
        setError(null);
      } catch (cause) {
        // The pin did not take, so the Target label must keep saying what is
        // actually being read.
        setError(describeIpcError(cause, UNKNOWN_FAILURE));
      } finally {
        if (bufferRef.current === buffer) setIsRetargeting(false);
      }
    },
    [projectId, transport],
  );

  return {
    items,
    isLoading,
    isLoadingEarlier,
    error,
    target,
    isRetargeting,
    loadEarlier,
    setTarget,
  };
}
