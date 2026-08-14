/**
 * "Load earlier" must not move what the reader is looking at (§6).
 *
 * These assertions are about the arithmetic only. jsdom reports every layout
 * measurement as 0, so a test that rendered `LogStream` and read `scrollTop`
 * would pass whatever the code did — the vacuous green this repo has been bitten
 * by before. The pixel behaviour stays a human check; what is pinned here is
 * that, given real measurements, the anchored row ends up at the same distance
 * from the top of the viewport as it started.
 */

import { describe, expect, it } from "vitest";

import { anchorCorrection, type ScrollAnchor } from "./scrollAnchor";

/** How far below the top of the viewport a row is drawn. */
function viewportOffset(rowTop: number, scrollTop: number): number {
  return rowTop - scrollTop;
}

describe("anchorCorrection", () => {
  it("leaves the anchored row at the same place on screen after a page is prepended", () => {
    // The reader is 400 px down, reading the row that opens the window.
    const anchor: ScrollAnchor = { id: "laravel.log:900", top: 480, scrollTop: 400 };
    const before = viewportOffset(anchor.top, anchor.scrollTop);

    // 300 Entries land above it, pushing it 7 200 px down the content.
    const afterPrepend = anchor.top + 7_200;
    const correction = anchorCorrection(anchor, afterPrepend);

    expect(correction).toEqual({ kind: "correct", scrollTop: 7_600 });
    if (correction.kind !== "correct") return;
    // The symptom: the row is drawn exactly where it was, not 7 200 px lower.
    expect(viewportOffset(afterPrepend, correction.scrollTop)).toBe(before);
  });

  it("keeps the row still whatever the size of the page that arrived", () => {
    const anchor: ScrollAnchor = { id: "laravel.log:900", top: 64, scrollTop: 2_048 };
    const before = viewportOffset(anchor.top, anchor.scrollTop);

    for (const prepended of [1, 19, 240, 12_000, 250_000]) {
      const correction = anchorCorrection(anchor, anchor.top + prepended);
      expect(correction.kind).toBe("correct");
      if (correction.kind !== "correct") continue;
      expect(viewportOffset(anchor.top + prepended, correction.scrollTop)).toBe(
        before,
      );
    }
  });

  it("does not touch the scroller when nothing moved", () => {
    // `load earlier` that pages in nothing — the file has no more history — must
    // not assign a scroll position at all. Assigning even the identical number
    // would cancel a smooth scroll the reader had in flight.
    const anchor: ScrollAnchor = { id: "laravel.log:900", top: 480, scrollTop: 400 };

    expect(anchorCorrection(anchor, 480)).toEqual({ kind: "unchanged" });
  });

  it("reports a vanished anchor rather than guessing where it went", () => {
    // The anchored row can disappear between the click and the page landing: a
    // filter change re-derives the matches, and the Session Record trims its
    // front at 2000. Inventing a position for a row that is gone would throw the
    // reader somewhere arbitrary — worse than the jump this exists to prevent.
    const anchor: ScrollAnchor = { id: "laravel.log:900", top: 480, scrollTop: 400 };

    expect(anchorCorrection(anchor, null)).toEqual({ kind: "unanchored" });
  });

  it("never asks for a negative scroll position", () => {
    // Only reachable if rows above the anchor were removed in the same commit.
    // A browser clamps a negative assignment to 0; saying so here keeps a
    // reader of this module from having to know that.
    const anchor: ScrollAnchor = { id: "laravel.log:900", top: 900, scrollTop: 40 };

    expect(anchorCorrection(anchor, 0)).toEqual({ kind: "correct", scrollTop: 0 });
  });
});
