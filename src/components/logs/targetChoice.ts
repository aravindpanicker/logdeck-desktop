/**
 * Reading a **Target** for the picker: which file it names, and which row in the
 * menu is the one currently being read.
 *
 * Pure, and in its own module for the same reason as `formatLogFile.ts` — D10
 * puts no component under test, so anything in the picker that can be *wrong*
 * rather than merely misdrawn is kept out of the component and pinned by
 * `targetChoice.test.ts`. `isCurrentTarget` is what decides `aria-checked`, so
 * a drift between it and `pinnedFile` would tell a screen-reader user they are
 * reading a file they are not.
 *
 * `Target` is externally tagged — `"latest"` or `{ file }` — as `watcher.rs`
 * serialises it (§3, and `target_serialises_as_the_shape_types_ts_already_assumes`).
 */

import type { LogFile, Target } from "../../lib/types";

/** The name of the pinned file, or `null` while following the newest one. */
export function pinnedFile(target: Target): string | null {
  return target === "latest" ? null : target.file;
}

/**
 * Whether `file` is what `target` is reading. `null` is the **Latest** row,
 * which is current exactly when nothing is pinned.
 */
export function isCurrentTarget(target: Target, file: LogFile | null): boolean {
  const pinned = pinnedFile(target);
  return file === null ? pinned === null : pinned === file.name;
}
