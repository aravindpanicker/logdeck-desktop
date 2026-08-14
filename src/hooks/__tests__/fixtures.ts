/**
 * Fixtures for the stream tests.
 *
 * Ids go through `toEntryId` / `toBreakId` / `toProjectId` rather than a bare
 * `as EntryId` cast: the brand exists to stop a ProjectId being used where an
 * EntryId belongs, and a cast at the fixture boundary would quietly defeat it.
 */

import {
  toBreakId,
  toEntryId,
  toProjectId,
  type Break,
  type BreakKind,
  type Level,
  type LogEntry,
  type ProjectId,
  type StreamItem,
} from "../../lib/types";

export const PROJECT: ProjectId = toProjectId("/Users/dev/api");

interface EntryOptions {
  readonly file?: string;
  readonly offset?: number;
  readonly level?: Level;
  readonly message?: string;
  readonly context?: string;
  readonly projectId?: ProjectId;
}

/**
 * Builds an Entry whose `raw` is the header plus its context, exactly as the
 * watcher captures it — search (D7) reads `raw`, so a fixture that left context
 * out of `raw` would make the search tests pass vacuously.
 */
export function makeEntry(options: EntryOptions = {}): LogEntry {
  const file = options.file ?? "laravel.log";
  const offset = options.offset ?? 0;
  const level = options.level ?? "error";
  const message = options.message ?? "Something failed";
  const context = options.context ?? "";
  const timestamp = "2026-08-14 01:28:00";
  const env = "local";
  const header = `[${timestamp}] ${env}.${level.toUpperCase()}: ${message}`;

  return {
    id: toEntryId(`${file}:${offset}`),
    projectId: options.projectId ?? PROJECT,
    file,
    offset,
    timestamp,
    env,
    level,
    message,
    context,
    raw: context === "" ? header : `${header}\n${context}`,
  };
}

export function makeBreak(
  offset: number,
  kind: BreakKind = "cleared",
  file = "laravel.log",
): Break {
  return {
    id: toBreakId(`${file}:${offset}:break`),
    projectId: PROJECT,
    kind,
    file,
  };
}

export function entryItem(entry: LogEntry): StreamItem {
  return { type: "entry", ...entry };
}

export function breakItem(brk: Break): StreamItem {
  return { type: "break", ...brk };
}

/** `count` Entries at increasing offsets, the shape the opening window returns. */
export function makeEntries(count: number, file = "laravel.log"): LogEntry[] {
  return Array.from({ length: count }, (_unused, position) =>
    makeEntry({ file, offset: position, message: `entry ${position}` }),
  );
}
