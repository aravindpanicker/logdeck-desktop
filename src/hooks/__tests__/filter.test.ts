import { describe, expect, it } from "vitest";

import {
  ALL_LEVELS_ACTIVE,
  filterItems,
  levelsAtOrAbove,
  matchesFilter,
  type StreamFilter,
} from "../../lib/filter";
import { isEntry, LEVELS, type Level } from "../../lib/types";
import { breakItem, entryItem, makeBreak, makeEntry } from "./fixtures";

function filter(overrides: Partial<StreamFilter> = {}): StreamFilter {
  return { levels: ALL_LEVELS_ACTIVE, query: "", ...overrides };
}

describe("levelsAtOrAbove", () => {
  it("derives a threshold set from severity rank, not alphabetical order", () => {
    const atError = levelsAtOrAbove("error");

    expect([...atError].sort()).toEqual(
      ["error", "critical", "alert", "emergency"].sort(),
    );
    // 'alert' and 'critical' sort before 'error' alphabetically but rank above it.
    expect(atError.has("alert")).toBe(true);
    expect(atError.has("warning")).toBe(false);
    expect(levelsAtOrAbove("unknown").size).toBe(LEVELS.length);
  });

  it("treats unknown as below debug, so a debug threshold excludes it", () => {
    // "unknown" is the absence of a severity, not the lowest one. A fragment
    // with no Monolog header must not arrive in a set the user asked to start
    // at DEBUG — and must not be able to inflate an Activity rollup either.
    const atDebug = levelsAtOrAbove("debug");

    expect(atDebug.has("unknown")).toBe(false);
    expect(atDebug.size).toBe(LEVELS.length - 1);
    expect(levelsAtOrAbove("unknown").has("unknown")).toBe(true);
  });
});

describe("matchesFilter — Level", () => {
  it("keeps an Entry whose Level is active and drops one whose Level is not", () => {
    const warning = entryItem(makeEntry({ level: "warning" }));
    const active = filter({ levels: levelsAtOrAbove("error") });

    expect(matchesFilter(warning, active)).toBe(false);
    expect(
      matchesFilter(entryItem(makeEntry({ level: "critical" })), active),
    ).toBe(true);
  });

  it("keeps a Break whatever the active Level set — a Break has no Level (D3)", () => {
    const marker = breakItem(makeBreak(10, "rotated"));

    expect(matchesFilter(marker, filter({ levels: new Set<Level>() }))).toBe(true);
    expect(
      matchesFilter(marker, filter({ levels: levelsAtOrAbove("emergency") })),
    ).toBe(true);
  });
});

describe("matchesFilter — search (D7)", () => {
  it("finds a term that appears ONLY in an Entry's context, never in its message", () => {
    const entry = makeEntry({
      message: "Unhandled exception",
      context:
        '{"userId":42}\n#0 /app/vendor/laravel/framework/src/Illuminate/Routing/Router.php(797): dispatchToRoute()',
    });
    expect(entry.message).not.toContain("dispatchToRoute");
    expect(entry.raw).toContain("dispatchToRoute");

    expect(matchesFilter(entryItem(entry), filter({ query: "dispatchToRoute" }))).toBe(
      true,
    );
  });

  it("matches case-insensitively against the whole raw Entry", () => {
    const entry = entryItem(
      makeEntry({ message: "Boom", context: "SQLSTATE[42S02]: Base table missing" }),
    );

    expect(matchesFilter(entry, filter({ query: "sqlstate" }))).toBe(true);
    expect(matchesFilter(entry, filter({ query: "  " }))).toBe(true);
    expect(matchesFilter(entry, filter({ query: "no such text" }))).toBe(false);
  });
});

describe("filterItems", () => {
  it("returns a new array and never mutates the source", () => {
    const items = [
      entryItem(makeEntry({ offset: 0, level: "debug", message: "noise" })),
      entryItem(makeEntry({ offset: 100, level: "error", message: "boom" })),
      breakItem(makeBreak(200)),
    ];
    const frozen = [...items];

    const result = filterItems(items, filter({ levels: levelsAtOrAbove("error") }));

    expect(result).not.toBe(items);
    expect(items).toEqual(frozen);
    expect(items).toHaveLength(3);
    expect(result).toHaveLength(2);
    expect(isEntry(result[0]) && result[0].message).toBe("boom");
  });

  it("applies Level and query together", () => {
    const items = [
      entryItem(makeEntry({ offset: 0, level: "error", message: "timeout" })),
      entryItem(makeEntry({ offset: 100, level: "error", message: "deadlock" })),
      entryItem(makeEntry({ offset: 200, level: "info", message: "timeout" })),
    ];

    const result = filterItems(
      items,
      filter({ levels: levelsAtOrAbove("error"), query: "timeout" }),
    );

    expect(result).toHaveLength(1);
    expect(isEntry(result[0]) && result[0].offset).toBe(0);
  });
});
