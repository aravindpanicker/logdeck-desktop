/**
 * The files available to pin as a **Target** (D5), for the picker in the
 * toolbar.
 *
 * **Fetched when the menu opens, never on mount.** A directory listing is stale
 * the moment it is taken — the `daily` channel creates tomorrow's file at
 * midnight and every file's size moves on every poll — so a list read at mount
 * would be wrong by the time anyone looked at it, and reading it eagerly for
 * every Project the user merely selects is a directory walk nobody asked for.
 * `refresh()` is what the picker calls as it opens.
 *
 * The failure is **surfaced, not swallowed**. A Project whose folder has been
 * moved away (D9) fails here rather than returning an empty list, and an empty
 * list would read as "this Project has no logs" — a read that failed is not a
 * state that is empty (LESSONS 2).
 */

import { useCallback, useEffect, useRef, useState } from "react";

import {
  describeIpcError,
  tauriTransport,
  type LogTransport,
} from "./transport";
import type { LogFile, ProjectId } from "../lib/types";

export interface UseLogFiles {
  /** Newest first, as `list_log_files` returns them. */
  readonly files: readonly LogFile[];
  readonly isLoading: boolean;
  /** The last failure from the bridge, verbatim where it is a string. */
  readonly error: string | null;
  /** Re-reads the directory. Called as the picker opens. */
  refresh(): Promise<void>;
}

const UNREADABLE = "The log directory could not be read";

/**
 * A `LogFile` as it arrives from the bridge, before the picker renders it.
 *
 * `invoke<LogFile[]>` asserts a shape rather than checking one. A reply that is
 * not an array would reach `files.map` in the picker and throw during render —
 * a blank window instead of a message — so it is refused here and surfaced as
 * the failure it is.
 */
function isLogFile(value: unknown): value is LogFile {
  if (typeof value !== "object" || value === null) return false;
  const file = value as {
    readonly name?: unknown;
    readonly bytes?: unknown;
    readonly modified?: unknown;
  };
  return (
    typeof file.name === "string" &&
    typeof file.bytes === "number" &&
    typeof file.modified === "number"
  );
}

function asListing(payload: unknown): LogFile[] {
  if (!Array.isArray(payload)) {
    throw new Error("list_log_files did not return a listing");
  }
  return payload.filter(isLogFile);
}

export function useLogFiles(
  projectId: ProjectId | null,
  transport: LogTransport = tauriTransport,
): UseLogFiles {
  const [files, setFiles] = useState<readonly LogFile[]>([]);
  const [isLoading, setIsLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  /**
   * The Project the newest request was made for. A listing that arrives after
   * the reader has moved on belongs to a directory they are no longer looking
   * at, and must not be shown beside another Project's name.
   */
  const requestedForRef = useRef<ProjectId | null>(projectId);

  useEffect(() => {
    requestedForRef.current = projectId;
    setFiles([]);
    setError(null);
    setIsLoading(false);
  }, [projectId]);

  const refresh = useCallback(async () => {
    if (projectId === null) return;
    requestedForRef.current = projectId;
    setIsLoading(true);
    try {
      const listing = await transport.invoke<unknown>("list_log_files", {
        projectId,
      });
      if (requestedForRef.current !== projectId) return;
      setFiles(asListing(listing));
      setError(null);
    } catch (cause) {
      if (requestedForRef.current !== projectId) return;
      // The previously listed files are dropped: they described a directory the
      // read just failed on, and showing them beside an error implies they are
      // still there.
      setFiles([]);
      setError(describeIpcError(cause, UNREADABLE));
    } finally {
      if (requestedForRef.current === projectId) setIsLoading(false);
    }
  }, [projectId, transport]);

  return { files, isLoading, error, refresh };
}
