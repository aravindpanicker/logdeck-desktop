/**
 * Where the rendered window starts within the filtered matches.
 *
 * The window renders the most recent N matches (§6). While the reader is pinned
 * to the bottom that window *slides*: as a match arrives, one row is added at
 * the bottom and one drops off the top, and the count stays at N.
 *
 * That sliding is wrong the moment the reader scrolls up. Dropping a row from
 * above the viewport shortens the content above them while `scrollTop` stays the
 * number it was, so everything shifts up and the reader is carried *down*
 * through the history by one row per arriving Entry. Nothing assigns
 * `scrollTop` — the content moves underneath them — which is why the pinning
 * logic can look entirely correct and the reader still gets dragged down.
 *
 * So while the lease is released, the window's START is what is held fixed and
 * the window grows downward instead. Rows above the viewport are never removed,
 * there is nothing to compensate for, and the reader stays where they are.
 * Growth is bounded by the Session Record's own 2000-item cap.
 */
export function windowStart(
  matchCount: number,
  size: number,
  /** Match count at the moment the reader scrolled up; `null` while pinned. */
  heldAt: number | null,
): number {
  const anchor = heldAt === null ? matchCount : Math.min(heldAt, matchCount);
  return Math.max(0, anchor - size);
}
