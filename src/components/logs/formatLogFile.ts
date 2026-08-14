/**
 * Display formatting for a `LogFile` row in the **Target** picker.
 *
 * Pure, and deliberately in its own module: everything else in this folder
 * needs a DOM to exercise, and D10 puts no component under test. These two
 * functions are the only part of the picker with arithmetic in it, so they are
 * the part worth pinning — see `formatLogFile.test.ts`.
 *
 * `modified` arrives as **epoch seconds** (`watcher.rs`, `LogFile`), and is
 * rendered as an age rather than a wall-clock date on purpose: an age needs no
 * locale and no timezone, so it means the same thing in a test as on screen.
 */

const BYTES_PER_STEP = 1024;
const BYTE_UNITS = ["B", "KB", "MB", "GB", "TB"] as const;

/** Below this many of a unit, a decimal place still carries information. */
const DECIMAL_BELOW = 10;

const SECONDS_PER_MINUTE = 60;
const SECONDS_PER_HOUR = 60 * SECONDS_PER_MINUTE;
const SECONDS_PER_DAY = 24 * SECONDS_PER_HOUR;
const SECONDS_PER_WEEK = 7 * SECONDS_PER_DAY;

/** Under this, "just now" is truer than a number that ticks while you read it. */
const JUST_NOW_SECONDS = 45;

/** A file's size, in the largest unit that leaves a number worth reading. */
export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 B";

  let value = bytes;
  let step = 0;
  while (value >= BYTES_PER_STEP && step < BYTE_UNITS.length - 1) {
    value /= BYTES_PER_STEP;
    step += 1;
  }

  // Whole bytes are never fractional; above that, one decimal until double
  // digits, where the decimal is noise beside the magnitude.
  const digits = step === 0 || value >= DECIMAL_BELOW ? 0 : 1;
  return `${value.toFixed(digits)} ${BYTE_UNITS[step]}`;
}

/**
 * How long ago a file was last written, from its epoch-seconds mtime.
 *
 * `nowSeconds` is a parameter rather than a `Date.now()` read so the function
 * stays pure. A future mtime — clock skew, or a file touched by a container on
 * a different clock — reads as "just now" rather than as a negative age.
 */
export function formatAge(modifiedSeconds: number, nowSeconds: number): string {
  if (!Number.isFinite(modifiedSeconds) || !Number.isFinite(nowSeconds)) {
    return "unknown";
  }

  const elapsed = nowSeconds - modifiedSeconds;
  if (elapsed < JUST_NOW_SECONDS) return "just now";
  if (elapsed < SECONDS_PER_HOUR) {
    return `${Math.floor(elapsed / SECONDS_PER_MINUTE)}m ago`;
  }
  if (elapsed < SECONDS_PER_DAY) {
    return `${Math.floor(elapsed / SECONDS_PER_HOUR)}h ago`;
  }
  if (elapsed < SECONDS_PER_WEEK) {
    return `${Math.floor(elapsed / SECONDS_PER_DAY)}d ago`;
  }
  return `${Math.floor(elapsed / SECONDS_PER_WEEK)}w ago`;
}

/** The one-line summary under a file's name: `12.4 KB · 3h ago`. */
export function describeLogFile(
  file: { readonly bytes: number; readonly modified: number },
  nowSeconds: number,
): string {
  return `${formatBytes(file.bytes)} · ${formatAge(file.modified, nowSeconds)}`;
}
