/**
 * How the sidebar interprets **Project** state.
 *
 * Pure functions only. Everything crossing IPC — the two event payloads
 * (`project:activity`, `project:status`) and the `Project` values returned by
 * `list_projects` (§3) — arrives from a phase built in parallel with this one,
 * so it is parsed defensively rather than trusted. `invoke<T>()` is a blind
 * cast at runtime, so a declared return type is not a check; these readers are.
 * A surprising payload is dropped, never rendered as `undefined` and never
 * thrown out of a listener.
 *
 * **Health** copy lives here too, because the sentence a user reads for an
 * unhealthy Project is behaviour (D4), not decoration.
 */

import {
  isUnavailable,
  LEVELS,
  toProjectId,
  type Health,
  type Level,
  type Project,
  type ProjectId,
} from "../../lib/types";

/* -------------------------------------------------------------------------- */
/* Activity                                                                    */
/* -------------------------------------------------------------------------- */

/**
 * What was written to a **Project** the user is not reading: how much, and how
 * bad. Never what — the text is discarded by the background watcher
 * (ADR 0002).
 */
export interface Activity {
  readonly total: number;
  readonly maxLevel: Level;
}

export interface ActivitySignal {
  readonly projectId: ProjectId;
  readonly activity: Activity;
}

export interface StatusSignal {
  readonly projectId: ProjectId;
  /** `null` means the Project is reachable again (D9). */
  readonly offlineReason: string | null;
}

/** Shown when a `project:status` offline payload carries no reason of its own. */
export const OFFLINE_FALLBACK_REASON = "source unavailable";

/** Counts above this are shown as `999+`; the badge is a signal, not a metric. */
const COUNT_DISPLAY_CAP = 999;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

/**
 * An unrecognised Level becomes `"unknown"` rather than being invented.
 *
 * `"unknown"` is the absence of a severity and ranks below `"debug"`, so a
 * payload we cannot read can never inflate the **Activity** rollup (D8).
 */
function readLevel(value: unknown): Level {
  return typeof value === "string" && (LEVELS as readonly string[]).includes(value)
    ? (value as Level)
    : "unknown";
}

function readProjectId(value: unknown): ProjectId | null {
  return typeof value === "string" && value.length > 0 ? toProjectId(value) : null;
}

/**
 * `total` is authoritative when present; `counts` is the fallback so a payload
 * that carries the per-Level breakdown but not the sum still renders.
 */
function readTotal(total: unknown, counts: unknown): number | null {
  if (typeof total === "number" && Number.isFinite(total) && total >= 0) {
    return Math.trunc(total);
  }
  if (!isRecord(counts)) {
    return null;
  }
  return Object.values(counts).reduce<number>(
    (sum, count) =>
      typeof count === "number" && Number.isFinite(count) && count > 0
        ? sum + Math.trunc(count)
        : sum,
    0,
  );
}

/**
 * Read a `project:activity` payload, or `null` if it is not one.
 *
 * The payload is a **snapshot** of the running total the backend holds since
 * the last `clear_activity`, not a delta — so the badge mirrors it rather than
 * accumulating, and a backend-side reset cannot leave a stale count on screen.
 */
export function readActivity(payload: unknown): ActivitySignal | null {
  if (!isRecord(payload)) {
    return null;
  }

  const projectId = readProjectId(payload.projectId);
  if (projectId === null) {
    return null;
  }

  const total = readTotal(payload.total, payload.counts);
  if (total === null) {
    return null;
  }

  return { projectId, activity: { total, maxLevel: readLevel(payload.maxLevel) } };
}

/**
 * Read a `project:status` payload, or `null` if it is not one.
 *
 * Only the literal state `"offline"` dims a **Project**. Any other state is
 * read as online, so a state this build has not heard of degrades to "fine"
 * rather than to a permanently dimmed row that never recovers.
 */
export function readStatus(payload: unknown): StatusSignal | null {
  if (!isRecord(payload) || typeof payload.state !== "string") {
    return null;
  }

  const projectId = readProjectId(payload.projectId);
  if (projectId === null) {
    return null;
  }

  if (payload.state !== "offline") {
    return { projectId, offlineReason: null };
  }

  const reason = typeof payload.reason === "string" ? payload.reason.trim() : "";
  return {
    projectId,
    offlineReason: reason.length > 0 ? reason : OFFLINE_FALLBACK_REASON,
  };
}

/* -------------------------------------------------------------------------- */
/* Project                                                                     */
/* -------------------------------------------------------------------------- */

/** The Health variants serde emits as bare strings. */
const HEALTH_NAMES: readonly string[] = ["ok", "noLogsDir", "notLaravel"];

/**
 * Read a `health` field, defaulting to `"ok"`.
 *
 * A variant this build has not heard of reads as healthy for the same reason an
 * unrecognised `project:status` state reads as online: the alternative is a row
 * permanently dimmed with a reason nobody can act on. `Unavailable(String)`
 * arrives as `{ unavailable: "…" }`, which is the one shape carrying a payload.
 */
function readHealth(value: unknown): Health {
  if (isRecord(value) && typeof value.unavailable === "string") {
    return { unavailable: value.unavailable };
  }
  if (typeof value === "string" && HEALTH_NAMES.includes(value)) {
    return value as Health;
  }
  return "ok";
}

/**
 * Read one **Project** from `list_projects`, or `null` if it has no identity.
 *
 * `id` is the only field that cannot be recovered — it is what `select_project`
 * and `remove_project` are keyed by, so a record without one is dropped rather
 * than rendered into a row whose buttons would call Rust with `undefined`.
 * `path` and `label` degrade instead of dropping: a Project the user registered
 * is always shown (D4), even if Rust stops deriving labels the way §8 says.
 */
export function readProject(value: unknown): Project | null {
  if (!isRecord(value)) {
    return null;
  }

  const id = readProjectId(value.id);
  if (id === null) {
    return null;
  }

  const path =
    typeof value.path === "string" && value.path.length > 0 ? value.path : id;
  const label =
    typeof value.label === "string" && value.label.trim().length > 0
      ? value.label.trim()
      : path;

  return { id, label, path, health: readHealth(value.health) };
}

/**
 * Read a whole `list_projects` result, or `null` if it is not a list at all.
 *
 * `null` is distinct from `[]`: an empty registry is a normal state the sidebar
 * has copy for, whereas a response that is not an array means the command is no
 * longer returning what §3 froze, and that is worth saying out loud.
 */
export function readProjects(payload: unknown): readonly Project[] | null {
  if (!Array.isArray(payload)) {
    return null;
  }
  return payload.reduce<Project[]>((kept, value) => {
    const project = readProject(value);
    return project === null ? kept : [...kept, project];
  }, []);
}

/* -------------------------------------------------------------------------- */
/* Health                                                                      */
/* -------------------------------------------------------------------------- */

/**
 * The sentence shown beneath an unhealthy **Project**, or `null` when there is
 * nothing to say.
 *
 * A real sentence, not a variant name: the row is the only place the user finds
 * out why a folder they registered is inert, and it is never hidden or rejected
 * for it (D4).
 */
export function describeHealth(health: Health): string | null {
  if (isUnavailable(health)) {
    const reason = health.unavailable.trim();
    return reason.length > 0 ? reason : OFFLINE_FALLBACK_REASON;
  }

  switch (health) {
    case "ok":
      return null;
    case "noLogsDir":
      return "no storage/logs found";
    case "notLaravel":
      return "not a Laravel project";
    default:
      return OFFLINE_FALLBACK_REASON;
  }
}

/**
 * Whether a **Project**'s row carries an **Activity** badge (D8).
 *
 * The badge counts what the user has *not* read, so selecting a Project clears
 * it, and a Project nothing has been written to has no badge rather than a `0`.
 * A rule about what a user sees, so it lives here with the rest of them and is
 * pinned by a test instead of only by a component that is never rendered in CI.
 */
export function shouldShowActivityBadge(
  isSelected: boolean,
  activity: Activity | undefined,
): activity is Activity {
  return !isSelected && activity !== undefined && activity.total > 0;
}

/** The count as the badge prints it. */
export function formatActivityCount(total: number): string {
  return total > COUNT_DISPLAY_CAP ? `${COUNT_DISPLAY_CAP}+` : String(total);
}

/** What a screen reader hears instead of the badge's two glyphs. */
export function describeActivity(activity: Activity): string {
  const entries = activity.total === 1 ? "1 new Entry" : `${activity.total} new Entries`;
  return activity.maxLevel === "unknown"
    ? `${entries}, no severity reported`
    : `${entries}, highest level ${activity.maxLevel}`;
}
