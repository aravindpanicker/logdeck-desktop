import { describe, expect, it } from "vitest";

import { windowStart } from "./streamWindow";

const SIZE = 300;

describe("windowStart", () => {
  it("slides with the newest matches while the reader is pinned", () => {
    // Pinned: the reader is at the bottom, so dropping a row from the top is
    // invisible to them and the window stays at its size.
    expect(windowStart(300, SIZE, null)).toBe(0);
    expect(windowStart(301, SIZE, null)).toBe(1);
    expect(windowStart(400, SIZE, null)).toBe(100);
  });

  it("renders everything when there are fewer matches than the window", () => {
    expect(windowStart(12, SIZE, null)).toBe(0);
    expect(windowStart(0, SIZE, null)).toBe(0);
  });

  it("holds the window's start while the reader is scrolled up", () => {
    // The regression this exists for: the reader scrolled up at 400 matches, so
    // the window started at row 100. Entries keep arriving. If the start moved
    // with them, each arrival would drop a row above the viewport and drag the
    // reader down by that row's height.
    const held = 400;
    expect(windowStart(400, SIZE, held)).toBe(100);
    expect(windowStart(401, SIZE, held)).toBe(100);
    expect(windowStart(450, SIZE, held)).toBe(100);
    expect(windowStart(2000, SIZE, held)).toBe(100);
  });

  it("grows the window downward rather than sliding it", () => {
    const held = 400;
    const start = windowStart(500, SIZE, held);
    // 100 rows of history above, and all 400 newer matches rendered below.
    expect(start).toBe(100);
    expect(500 - start).toBe(400);
  });

  it("resumes sliding once the reader returns to the bottom", () => {
    expect(windowStart(500, SIZE, 400)).toBe(100);
    expect(windowStart(500, SIZE, null)).toBe(200);
  });

  it("survives the match count shrinking under a held window", () => {
    // A narrower filter can leave fewer matches than there were when the reader
    // scrolled up. The start must not run past the end of what exists.
    expect(windowStart(50, SIZE, 400)).toBe(0);
    expect(windowStart(0, SIZE, 400)).toBe(0);
  });

  it("moves the start back when the reader pages earlier", () => {
    // 'Load earlier' grows the size, which is the one way the start may move up
    // while held — it reveals history above without disturbing what is on screen.
    expect(windowStart(400, SIZE, 400)).toBe(100);
    expect(windowStart(400, SIZE * 2, 400)).toBe(0);
  });
});
