/**
 * The two live signals the sidebar renders: **Activity** and online/offline.
 *
 * Both are ephemeral session state and deliberately not persisted — reopening
 * the app shows a quiet sidebar, not a backlog (ADR 0002).
 *
 * The emitting phase is built in parallel with this one, so nothing here
 * assumes an event ever arrives: with no events at all the maps stay empty and
 * every row renders from its registry **Health** alone.
 */

import { useCallback, useEffect, useRef, useState } from "react";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import type { ProjectId } from "../../lib/types";
import {
  readActivity,
  readStatus,
  type Activity,
} from "./projectSignals";

export interface ProjectSignals {
  /** **Activity** per **Project**; absent means nothing has arrived. */
  readonly activity: ReadonlyMap<ProjectId, Activity>;
  /** Offline reason per **Project**; absent means reachable. */
  readonly offline: ReadonlyMap<ProjectId, string>;
  /**
   * Set when the event bridge itself could not be reached — in a plain browser
   * (`npm run dev`) there is no Tauri runtime. Surfaced rather than swallowed,
   * because the alternative is a sidebar that silently never updates.
   */
  readonly signalError: string | null;
}

function withEntry<V>(
  map: ReadonlyMap<ProjectId, V>,
  key: ProjectId,
  value: V,
): ReadonlyMap<ProjectId, V> {
  const next = new Map(map);
  next.set(key, value);
  return next;
}

function withoutEntry<V>(
  map: ReadonlyMap<ProjectId, V>,
  key: ProjectId,
): ReadonlyMap<ProjectId, V> {
  if (!map.has(key)) {
    return map;
  }
  const next = new Map(map);
  next.delete(key);
  return next;
}

/**
 * @param selectedProjectId The **Project** currently being read, whose Activity
 * is by definition zero — the user is looking at it.
 */
export function useProjectSignals(
  selectedProjectId: ProjectId | null,
): ProjectSignals {
  const [activity, setActivity] = useState<ReadonlyMap<ProjectId, Activity>>(
    new Map(),
  );
  const [offline, setOffline] = useState<ReadonlyMap<ProjectId, string>>(
    new Map(),
  );
  const [signalError, setSignalError] = useState<string | null>(null);

  // Read inside the listeners without re-subscribing on every selection change;
  // resubscribing would drop events in the gap between unlisten and listen.
  const selectedRef = useRef(selectedProjectId);
  selectedRef.current = selectedProjectId;

  const onActivity = useCallback((payload: unknown): void => {
    const signal = readActivity(payload);
    if (signal === null || signal.projectId === selectedRef.current) {
      // A Project the user is reading has no Activity to report (D8).
      return;
    }
    setActivity((current) => withEntry(current, signal.projectId, signal.activity));
  }, []);

  const onStatus = useCallback((payload: unknown): void => {
    const signal = readStatus(payload);
    if (signal === null) {
      return;
    }
    setOffline((current) =>
      signal.offlineReason === null
        ? withoutEntry(current, signal.projectId)
        : withEntry(current, signal.projectId, signal.offlineReason),
    );
  }, []);

  useEffect(() => {
    let cancelled = false;
    const unlisteners: UnlistenFn[] = [];

    const attach = async (event: string, handler: (payload: unknown) => void) => {
      const unlisten = await listen<unknown>(event, (received) =>
        handler(received.payload),
      );
      if (cancelled) {
        unlisten();
        return;
      }
      unlisteners.push(unlisten);
    };

    Promise.all([
      attach("project:activity", onActivity),
      attach("project:status", onStatus),
    ]).catch((cause: unknown) => {
      if (!cancelled) {
        setSignalError(
          cause instanceof Error ? cause.message : "live updates are unavailable",
        );
      }
    });

    return () => {
      cancelled = true;
      for (const unlisten of unlisteners) {
        unlisten();
      }
    };
  }, [onActivity, onStatus]);

  // Selecting a Project clears its badge (D8). The backend's own counter is
  // reset by `clear_activity`, which belongs to the phase that owns
  // `select_project` — this drop is what the user sees, and the guard in
  // `onActivity` keeps a late event from putting the badge back.
  useEffect(() => {
    if (selectedProjectId === null) {
      return;
    }
    setActivity((current) => withoutEntry(current, selectedProjectId));
  }, [selectedProjectId]);

  return { activity, offline, signalError };
}
