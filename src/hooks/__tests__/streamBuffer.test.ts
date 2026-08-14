import { describe, expect, it } from "vitest";

import { createStreamBuffer, STREAM_CAP } from "../streamBuffer";
import { isBreak, isEntry } from "../../lib/types";
import { breakItem, entryItem, makeBreak, makeEntries, makeEntry } from "./fixtures";

describe("streamBuffer — upsert by id (D2)", () => {
  it("replaces an Entry with an existing id in place, keeping length and position", () => {
    const buffer = createStreamBuffer();
    buffer.upsertEntry(makeEntry({ offset: 0, message: "first" }));
    buffer.upsertEntry(makeEntry({ offset: 100, message: "second" }));
    buffer.upsertEntry(makeEntry({ offset: 200, message: "third" }));

    const revised = makeEntry({ offset: 100, message: "second (revised)" });
    buffer.upsertEntry(revised);

    const items = buffer.snapshot();
    expect(items).toHaveLength(3);
    expect(buffer.positionOf(revised.id)).toBe(1);

    const atPosition = items[1];
    expect(isEntry(atPosition)).toBe(true);
    expect(isEntry(atPosition) && atPosition.message).toBe("second (revised)");
    // The Entries either side are untouched.
    expect(isEntry(items[0]) && items[0].message).toBe("first");
    expect(isEntry(items[2]) && items[2].message).toBe("third");
  });

  it("keeps the position of a revised Entry that grew, so a late stack trace does not reorder the view", () => {
    const buffer = createStreamBuffer();
    buffer.upsertEntry(makeEntry({ offset: 0, message: "earlier" }));
    const torn = makeEntry({ offset: 100, message: "boom" });
    buffer.upsertEntry(torn);
    buffer.upsertEntry(makeEntry({ offset: 300, message: "later" }));

    const trace = Array.from(
      { length: 47 },
      (_unused, frame) => `#${frame} /app/vendor/frame.php(${frame}): call()`,
    ).join("\n");
    const grown = makeEntry({ offset: 100, message: "boom", context: trace });
    expect(grown.raw.length).toBeGreaterThan(torn.raw.length);

    buffer.upsertEntry(grown);

    const items = buffer.snapshot();
    expect(items).toHaveLength(3);
    expect(buffer.positionOf(grown.id)).toBe(1);
    expect(isEntry(items[1]) && items[1].context).toBe(trace);
    // The trailing Entry stays trailing.
    expect(isEntry(items[2]) && items[2].message).toBe("later");
  });
});

describe("streamBuffer — Break (D3, ADR 0001)", () => {
  it("appends a Break marker and removes nothing above it", () => {
    const buffer = createStreamBuffer();
    const before = makeEntries(5);
    for (const entry of before) buffer.upsertEntry(entry);

    const marker = makeBreak(999, "cleared");
    buffer.appendBreak(marker);

    const items = buffer.snapshot();
    expect(items).toHaveLength(6);
    expect(items.slice(0, 5)).toEqual(before.map(entryItem));
    expect(isBreak(items[5]) && items[5].kind).toBe("cleared");

    // Entries appended after the Break sit below it; the ones above survive.
    buffer.upsertEntry(makeEntry({ offset: 1000, message: "after the break" }));
    const grown = buffer.snapshot();
    expect(grown).toHaveLength(7);
    expect(grown.slice(0, 5)).toEqual(before.map(entryItem));
  });

  it("ignores a re-delivered Break with an id already held", () => {
    const buffer = createStreamBuffer();
    for (const entry of makeEntries(3)) buffer.upsertEntry(entry);

    const marker = makeBreak(999, "cleared");
    buffer.appendBreak(marker);
    // The watcher can re-emit the same marker; a duplicate must not show twice.
    buffer.appendBreak(marker);

    const items = buffer.snapshot();
    expect(items).toHaveLength(4);
    expect(buffer.positionOf(marker.id)).toBe(3);
    expect(items.filter(isBreak)).toHaveLength(1);
  });
});

describe("streamBuffer — cap", () => {
  it("caps at 2000 by trimming from the front", () => {
    const buffer = createStreamBuffer();
    for (const entry of makeEntries(STREAM_CAP + 10)) buffer.upsertEntry(entry);

    const items = buffer.snapshot();
    expect(STREAM_CAP).toBe(2000);
    expect(items).toHaveLength(STREAM_CAP);
    // The oldest ten were dropped; the newest survived.
    expect(isEntry(items[0]) && items[0].message).toBe("entry 10");
    const newest = items[STREAM_CAP - 1];
    expect(isEntry(newest) && newest.message).toBe(`entry ${STREAM_CAP + 9}`);
  });

  it("keeps the index consistent after trimming — a later upsert of a surviving id still lands in place", () => {
    const buffer = createStreamBuffer();
    for (const entry of makeEntries(STREAM_CAP + 10)) buffer.upsertEntry(entry);

    // Offset 10 is now the first surviving Entry, i.e. position 0.
    const survivor = makeEntry({ offset: 10, message: "entry 10" });
    expect(buffer.positionOf(survivor.id)).toBe(0);

    buffer.upsertEntry(makeEntry({ offset: 10, message: "entry 10 (revised)" }));

    const items = buffer.snapshot();
    expect(items).toHaveLength(STREAM_CAP);
    expect(buffer.positionOf(survivor.id)).toBe(0);
    expect(isEntry(items[0]) && items[0].message).toBe("entry 10 (revised)");
    // A trimmed id is gone from the index, not left pointing at a stale slot.
    const trimmed = makeEntry({ offset: 0, message: "entry 0" });
    expect(buffer.positionOf(trimmed.id)).toBeUndefined();
  });

  it("keeps the index consistent for an Entry in the middle after trimming", () => {
    const buffer = createStreamBuffer();
    for (const entry of makeEntries(STREAM_CAP + 10)) buffer.upsertEntry(entry);

    const middle = makeEntry({ offset: 1010, message: "entry 1010" });
    const position = buffer.positionOf(middle.id);
    expect(position).toBe(1000);

    buffer.upsertEntry(makeEntry({ offset: 1010, message: "entry 1010 (revised)" }));
    const items = buffer.snapshot();
    expect(items).toHaveLength(STREAM_CAP);
    expect(isEntry(items[1000]) && items[1000].message).toBe("entry 1010 (revised)");
  });
});

describe("streamBuffer — load_earlier", () => {
  it("prepends and keeps the index consistent", () => {
    const buffer = createStreamBuffer();
    const live = makeEntries(3).map((entry) =>
      makeEntry({ offset: entry.offset + 500, message: entry.message }),
    );
    for (const entry of live) buffer.upsertEntry(entry);

    const earlier = [
      makeEntry({ offset: 10, message: "older a" }),
      makeEntry({ offset: 20, message: "older b" }),
    ];
    buffer.prepend(earlier.map(entryItem));

    const items = buffer.snapshot();
    expect(items).toHaveLength(5);
    expect(isEntry(items[0]) && items[0].message).toBe("older a");
    expect(isEntry(items[1]) && items[1].message).toBe("older b");
    expect(buffer.positionOf(earlier[0].id)).toBe(0);
    expect(buffer.positionOf(live[0].id)).toBe(2);

    // Positions shifted by the prepend, and an upsert still lands on the right slot.
    buffer.upsertEntry(
      makeEntry({ offset: live[0].offset, message: "revised live head" }),
    );
    const revised = buffer.snapshot();
    expect(revised).toHaveLength(5);
    const head = revised[2];
    expect(isEntry(head) && head.message).toBe("revised live head");
  });

  it("does not re-deliver an Entry that is already held", () => {
    const buffer = createStreamBuffer();
    const entry = makeEntry({ offset: 10, message: "already here" });
    buffer.upsertEntry(entry);

    buffer.prepend([entryItem(entry), breakItem(makeBreak(5, "rotated"))]);

    const items = buffer.snapshot();
    expect(items).toHaveLength(2);
    expect(isBreak(items[0])).toBe(true);
    expect(buffer.positionOf(entry.id)).toBe(1);
  });

  it("does not trim the page it was just asked for, and re-applies the cap on the next append", () => {
    const buffer = createStreamBuffer();
    buffer.upsertEntry(makeEntry({ offset: 9_000_000, message: "live head" }));

    // A page larger than the cap: trimming here would discard the newly paged
    // Entries the user explicitly asked for, so `prepend` deliberately does not.
    const page = makeEntries(STREAM_CAP + 10).map(entryItem);
    buffer.prepend(page);
    expect(buffer.size()).toBe(STREAM_CAP + 11);

    // The cap re-asserts itself as soon as live Entries arrive again.
    buffer.upsertEntry(makeEntry({ offset: 9_000_100, message: "next live" }));
    expect(buffer.size()).toBe(STREAM_CAP);

    const items = buffer.snapshot();
    const newest = items[STREAM_CAP - 1];
    expect(isEntry(newest) && newest.message).toBe("next live");
    // Front-trimming dropped the oldest of the paged Entries, index and all.
    expect(buffer.positionOf(page[0].id)).toBeUndefined();
    expect(buffer.positionOf(page[page.length - 1].id)).toBe(STREAM_CAP - 3);
  });
});

describe("streamBuffer — snapshot", () => {
  it("hands out a copy, so a caller cannot corrupt the buffer", () => {
    const buffer = createStreamBuffer();
    buffer.upsertEntry(makeEntry({ offset: 0 }));

    const first = buffer.snapshot();
    first.length = 0;

    expect(buffer.snapshot()).toHaveLength(1);
  });

  it("returns a fresh array each call, so React sees a new identity after a change", () => {
    const buffer = createStreamBuffer();
    buffer.upsertEntry(makeEntry({ offset: 0 }));
    const before = buffer.snapshot();
    buffer.upsertEntry(makeEntry({ offset: 100 }));
    const after = buffer.snapshot();

    expect(after).not.toBe(before);
    expect(before).toHaveLength(1);
    expect(after).toHaveLength(2);
  });
});
