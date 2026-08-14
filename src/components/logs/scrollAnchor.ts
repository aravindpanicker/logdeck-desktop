/**
 * Where the scroller has to be put after an earlier page lands.
 *
 * "Load earlier" inserts Entries **above** everything on screen. The content
 * above the viewport therefore gets taller while `scrollTop` stays the number
 * it was, so the rows the reader was looking at are pushed down by exactly the
 * height of the page that arrived. Nothing scrolls — the content moves under
 * them — and what the reader sees is the line they were reading leaping down
 * the screen, usually far enough to lose it entirely. That is the jump §6 says
 * must not happen.
 *
 * The fix is to measure one row before the page lands and put the scroller back
 * by however far that row moved. This module is only that arithmetic, kept pure
 * for the same reason `streamWindow.ts` is: the correction can be reasoned about
 * and tested, while the measuring — `offsetTop`, `scrollTop` — stays in the
 * layout effect that owns the DOM.
 *
 * **What this module cannot prove.** jsdom has no layout engine: `offsetTop`,
 * `offsetHeight`, `scrollHeight` and `clientHeight` are all 0 there. So the
 * numbers below are verified, and the pixels are not. Manufacturing fake
 * layout metrics would make the test green without making it true; the visual
 * half stays a human check (LESSONS, Manual verification item 6).
 */

/** Where the first rendered item sat before an earlier page was inserted. */
export interface ScrollAnchor {
  readonly id: string;
  /** The row's `offsetTop` at the moment of measurement. */
  readonly top: number;
  /** The scroller's `scrollTop` at the moment of measurement. */
  readonly scrollTop: number;
}

/**
 * What the layout effect should do with the scroller.
 *
 * Three outcomes rather than a nullable number, because "leave it alone" and
 * "this anchor is gone" are different instructions: the second one has to fall
 * through to the auto-scroll lease, and the first one must not.
 */
export type AnchorCorrection =
  /** Assign this `scrollTop`; the anchored row moved by that much. */
  | { readonly kind: "correct"; readonly scrollTop: number }
  /** The anchored row did not move. Assigning anything would be the bug. */
  | { readonly kind: "unchanged" }
  /** The anchored row is no longer rendered; the caller decides what to do. */
  | { readonly kind: "unanchored" };

/**
 * @param anchor Where the anchored row sat before the page arrived.
 * @param anchoredRowTop The same row's `offsetTop` now, or `null` if that row
 * is no longer in the scroller — a filter change or a front-trim of the
 * **Session Record** can remove it between the click and the page landing.
 * Guessing a position for a row that is gone would scroll the reader somewhere
 * arbitrary, so this reports the loss instead of inventing a number.
 */
export function anchorCorrection(
  anchor: ScrollAnchor,
  anchoredRowTop: number | null,
): AnchorCorrection {
  if (anchoredRowTop === null) {
    return { kind: "unanchored" };
  }

  const moved = anchoredRowTop - anchor.top;
  if (moved === 0) {
    return { kind: "unchanged" };
  }

  // Clamped because a browser clamps it anyway, and a negative `scrollTop`
  // assigned here would read as a bug in this arithmetic rather than as the
  // no-op the DOM turns it into.
  return { kind: "correct", scrollTop: Math.max(0, anchor.scrollTop + moved) };
}
