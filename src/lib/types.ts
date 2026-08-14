/**
 * TypeScript mirror of `src-tauri/src/model.rs`.
 *
 * Every Rust payload type carries `#[serde(rename_all = "camelCase")]`, so
 * `project_id` on the wire is `projectId` here. A field that is misnamed or
 * missing on this side compiles cleanly and fails silently at runtime — this
 * file is checked field-for-field against BUILD-SPEC.md §2.
 */

/* -------------------------------------------------------------------------- */
/* Identity                                                                    */
/* -------------------------------------------------------------------------- */

/**
 * Three distinct identity spaces, branded so the compiler can tell them apart.
 *
 * On the wire they are ordinary strings — the Rust newtypes are
 * `#[serde(transparent)]` — so the frozen IPC contract in §3 is unaffected.
 * The brand exists because the stream is keyed by `EntryId`; putting a
 * `ProjectId` into that map is a live bug, and this makes it a compile error.
 */
export type ProjectId = string & { readonly __brand: "ProjectId" };
export type EntryId = string & { readonly __brand: "EntryId" };
export type BreakId = string & { readonly __brand: "BreakId" };

/**
 * The only sanctioned way to brand a raw string.
 *
 * Values arriving over IPC are already branded by the declared return types, so
 * these exist for the places that hold a bare string — test fixtures, and any
 * id read back out of the DOM or a route. One helper per identity space keeps
 * ad-hoc `as EntryId` casts, which would defeat the brand, out of the codebase.
 */
export function toProjectId(value: string): ProjectId {
  return value as ProjectId;
}

export function toEntryId(value: string): EntryId {
  return value as EntryId;
}

export function toBreakId(value: string): BreakId {
  return value as BreakId;
}

/* -------------------------------------------------------------------------- */
/* Level                                                                       */
/* -------------------------------------------------------------------------- */

/**
 * The severity a Laravel **Entry** was written at.
 *
 * Declared least- to most-severe, matching the `Ord` derive on the Rust enum.
 *
 * `"unknown"` means no severity rather than a low one: content that never
 * announced a Level, such as a PHP fatal or stderr redirected into the log. It
 * ranks below `"debug"` so it can never inflate the **Activity** rollup, and it
 * is a distinct value so a fragment does not impersonate a real INFO Entry —
 * which would also hide it behind an INFO filter.
 */
export type Level =
  | "unknown"
  | "debug"
  | "info"
  | "notice"
  | "warning"
  | "error"
  | "critical"
  | "alert"
  | "emergency";

/**
 * The nine Levels in severity order — the eight PSR severities, plus `unknown`
 * for content that never announced one, which ranks below all of them.
 *
 * Rust gets ordering from `#[derive(Ord)]`; JS has no such thing for a string
 * union, so severity comparison here goes through this array's index. The
 * **Activity** rollup (highest Level across a batch, D8) and filter thresholds
 * both depend on it.
 */
export const LEVELS: readonly Level[] = [
  "unknown",
  "debug",
  "info",
  "notice",
  "warning",
  "error",
  "critical",
  "alert",
  "emergency",
] as const;

/** Rank of a Level in severity order; higher is more severe. */
export function levelRank(level: Level): number {
  return LEVELS.indexOf(level);
}

/* -------------------------------------------------------------------------- */
/* Health                                                                      */
/* -------------------------------------------------------------------------- */

/**
 * Whether a **Project** can currently be read, and if not, why.
 *
 * Serde emits unit variants as bare strings and the payload-carrying variant as
 * a single-key object, so `Unavailable(String)` arrives as
 * `{ unavailable: "..." }`. An unhealthy Project is still registered (D4).
 */
export type Health =
  | "ok"
  | "noLogsDir"
  | "notLaravel"
  | { readonly unavailable: string };

/** Narrows the one Health variant that carries a reason. */
export function isUnavailable(
  health: Health,
): health is { readonly unavailable: string } {
  return typeof health === "object" && "unavailable" in health;
}

/* -------------------------------------------------------------------------- */
/* Project                                                                     */
/* -------------------------------------------------------------------------- */

/** A folder the user has registered, identified by its absolute path. */
export interface Project {
  /** The canonicalized absolute path. */
  readonly id: ProjectId;
  /** Basename; the parent segment is appended when two Projects collide. */
  readonly label: string;
  readonly path: string;
  readonly health: Health;
}

/* -------------------------------------------------------------------------- */
/* Stream                                                                      */
/* -------------------------------------------------------------------------- */

/**
 * One logical log event: a header together with the context and stack frames
 * beneath it.
 */
export interface LogEntry {
  readonly id: EntryId;
  readonly projectId: ProjectId;
  readonly file: string;
  readonly offset: number;
  /** Verbatim as parsed, not normalised. */
  readonly timestamp: string;
  readonly env: string;
  readonly level: Level;
  /** Header remainder, first line only. */
  readonly message: string;
  /** Following lines, joined with `\n`. */
  readonly context: string;
  /** Full verbatim text including the header — what Copy sends (D1). */
  readonly raw: string;
}

/** Why a **Break** was inserted into the **Session Record**. */
export type BreakKind = "cleared" | "rotated";

/**
 * A point in the **Session Record** where the underlying source discontinued.
 * Entries either side of a Break are unrelated in time.
 */
export interface Break {
  readonly id: BreakId;
  readonly projectId: ProjectId;
  readonly kind: BreakKind;
  /** The file in effect *after* the Break. */
  readonly file: string;
}

/**
 * What the stream carries over IPC.
 *
 * The Rust enum is internally tagged on `type`, so the variant's own fields sit
 * on the same object rather than under a wrapper key.
 */
export type StreamItem =
  | ({ readonly type: "entry" } & LogEntry)
  | ({ readonly type: "break" } & Break);

/** Narrows a StreamItem to its Entry arm. */
export function isEntry(
  item: StreamItem,
): item is { readonly type: "entry" } & LogEntry {
  return item.type === "entry";
}

/** Narrows a StreamItem to its Break arm. */
export function isBreak(
  item: StreamItem,
): item is { readonly type: "break" } & Break {
  return item.type === "break";
}

/* -------------------------------------------------------------------------- */
/* Files                                                                       */
/* -------------------------------------------------------------------------- */

/**
 * These two were written here first, as an assumption, before any Rust
 * counterpart existed. They are no longer an assumption: `src-tauri/src/
 * watcher.rs` declares both, matching these shapes exactly, and pins them with
 * round-trip tests — `target_serialises_as_the_shape_types_ts_already_assumes`
 * and `log_file_serialises_as_the_shape_types_ts_already_assumes`.
 *
 * `watcher.rs` is the authority; change these only together with it. The two
 * facts those tests exist to hold down: `modified` is **epoch seconds**, not an
 * RFC 3339 string, and `Target` is **externally tagged** — `"latest"` /
 * `{ "file": "…" }`, never internally tagged or PascalCase. A divergence
 * compiles cleanly on both sides and fails only when the command is invoked.
 *
 * Both commands are now reached from the UI through `TargetPicker`. They were
 * implemented, registered and tested well before anything called them, which is
 * how the gap in LESSON 6 went unnoticed — a command, a caller and a
 * verification row all have to agree before a feature is shipped.
 */

/**
 * One file inside a Project's `storage/logs/`, as returned by `list_log_files`
 * (§3). A Project may hold `laravel.log`, dated `laravel-YYYY-MM-DD.log` files,
 * or both, so the **Target** picker lists whatever is actually there.
 */
export interface LogFile {
  readonly name: string;
  readonly bytes: number;
  /** Last-modified time, seconds since the Unix epoch. */
  readonly modified: number;
}

/**
 * The **Target** a Project is reading: the newest file, followed as it changes,
 * or one file the user has pinned (D5).
 */
export type Target = "latest" | { readonly file: string };
