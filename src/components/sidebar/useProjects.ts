/**
 * The **Project** registry, as the sidebar sees it.
 *
 * Rust owns the registry (BUILD-SPEC §8): it holds the watchers, it persists
 * the list, and it derives labels across the whole registry — adding one
 * Project can relabel another when their basenames collide. So this hook keeps
 * no client-side model of its own. Every mutation is followed by a re-read of
 * `list_projects`, which is the only thing that cannot drift.
 *
 * Lives under `components/sidebar/` rather than `hooks/` because `src/hooks/**`
 * belongs to another phase.
 */

import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

import type { Project, ProjectId } from "../../lib/types";
import { readProjects } from "./projectSignals";

export interface UseProjects {
  readonly projects: readonly Project[];
  /** True until the first `list_projects` settles. */
  readonly isLoading: boolean;
  /** True while an add or a remove is in flight. */
  readonly isBusy: boolean;
  /** The last failure, verbatim from Rust. `null` once dismissed. */
  readonly error: string | null;
  readonly addProject: (path: string) => Promise<void>;
  readonly removeProject: (projectId: ProjectId) => Promise<void>;
  readonly reportError: (message: string) => void;
  readonly dismissError: () => void;
}

/**
 * Every command in §3 returns `Result<_, String>`, so a rejection carries the
 * Rust-side sentence — `project path must be absolute`, `cannot resolve …`, or
 * the refusal to write a registry that could not be read. That text is the most
 * useful thing we can show, so it is surfaced rather than replaced.
 */
export function commandError(error: unknown): string {
  if (typeof error === "string" && error.trim().length > 0) {
    return error.trim();
  }
  if (error instanceof Error && error.message.trim().length > 0) {
    return error.message.trim();
  }
  return "the command failed without saying why";
}

/** Shown when `list_projects` answers with something that is not a list. */
export const UNREADABLE_LIST_ERROR =
  "the project list could not be read — the app and its backend disagree";

export function useProjects(): UseProjects {
  const [projects, setProjects] = useState<readonly Project[]>([]);
  const [isLoading, setIsLoading] = useState(true);
  const [isBusy, setIsBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // The hook outlives an in-flight command when the window closes; writing
  // state after that is a React warning and, worse, hides the real failure.
  const isMounted = useRef(true);
  useEffect(() => {
    isMounted.current = true;
    return () => {
      isMounted.current = false;
    };
  }, []);

  const refresh = useCallback(async (): Promise<void> => {
    try {
      // `invoke<T>()` asserts rather than checks — the shape is whatever Rust
      // actually sent — so the answer is read through the same defensive parser
      // the event payloads go through, not cast.
      const listed = readProjects(await invoke("list_projects"));
      if (!isMounted.current) {
        return;
      }
      if (listed === null) {
        setError(UNREADABLE_LIST_ERROR);
        return;
      }
      setProjects(listed);
    } catch (cause: unknown) {
      if (isMounted.current) {
        setError(commandError(cause));
      }
    }
  }, []);

  useEffect(() => {
    void refresh().finally(() => {
      if (isMounted.current) {
        setIsLoading(false);
      }
    });
  }, [refresh]);

  const addProject = useCallback(
    async (path: string): Promise<void> => {
      setIsBusy(true);
      setError(null);
      try {
        // The returned Project is deliberately discarded — and so left
        // untyped, since a cast we never read would only be a lie: a successful
        // add can change another Project's label, so the list is re-read rather
        // than patched. An unhealthy folder still succeeds here (D4).
        await invoke("add_project", { path });
        await refresh();
      } catch (cause: unknown) {
        if (isMounted.current) {
          setError(commandError(cause));
        }
      } finally {
        if (isMounted.current) {
          setIsBusy(false);
        }
      }
    },
    [refresh],
  );

  const removeProject = useCallback(
    async (projectId: ProjectId): Promise<void> => {
      setIsBusy(true);
      setError(null);
      try {
        await invoke("remove_project", { projectId });
        await refresh();
      } catch (cause: unknown) {
        if (isMounted.current) {
          setError(commandError(cause));
        }
      } finally {
        if (isMounted.current) {
          setIsBusy(false);
        }
      }
    },
    [refresh],
  );

  const reportError = useCallback((message: string): void => {
    setError(message);
  }, []);

  const dismissError = useCallback((): void => {
    setError(null);
  }, []);

  return {
    projects,
    isLoading,
    isBusy,
    error,
    addProject,
    removeProject,
    reportError,
    dismissError,
  };
}
