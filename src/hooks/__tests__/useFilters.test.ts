import { act, cleanup, renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { useFilters } from "../useFilters";
import { isEntry, type StreamItem } from "../../lib/types";
import { entryItem, makeEntry } from "./fixtures";

afterEach(cleanup);

const ITEMS: StreamItem[] = [
  entryItem(makeEntry({ offset: 0, level: "debug", message: "booting" })),
  entryItem(makeEntry({ offset: 100, level: "info", message: "request handled" })),
  entryItem(
    makeEntry({
      offset: 200,
      level: "error",
      message: "Unhandled exception",
      context: "#0 /app/vendor/framework/Router.php(797): dispatchToRoute()",
    }),
  ),
];

describe("useFilters — derivation, never a second buffer", () => {
  it("changing the filter does not mutate items and leaves items.length unchanged", () => {
    const items = [...ITEMS];
    const { result } = renderHook(() => useFilters(items));

    expect(result.current.filtered).toHaveLength(3);

    act(() => result.current.setQuery("exception"));

    expect(items).toHaveLength(3);
    expect(items).toEqual(ITEMS);
    expect(result.current.filtered).toHaveLength(1);

    act(() => result.current.setMinLevel("emergency"));
    expect(items).toHaveLength(3);
    expect(items).toEqual(ITEMS);
  });

  it("keeps the derived array identity stable while the inputs are unchanged", () => {
    const { result, rerender } = renderHook(() => useFilters(ITEMS));
    const first = result.current.filtered;

    rerender();

    expect(result.current.filtered).toBe(first);
  });

  it("recomputes from items, so a filtered view never drifts from the source", () => {
    const { result, rerender } = renderHook(({ items }) => useFilters(items), {
      initialProps: { items: ITEMS },
    });

    act(() => result.current.setMinLevel("error"));
    expect(result.current.filtered).toHaveLength(1);

    const grown = [
      ...ITEMS,
      entryItem(makeEntry({ offset: 300, level: "critical", message: "oom" })),
    ];
    rerender({ items: grown });

    expect(result.current.filtered).toHaveLength(2);
    expect(isEntry(result.current.filtered[1]) && result.current.filtered[1].message).toBe(
      "oom",
    );
  });
});

describe("useFilters — search (D7)", () => {
  it("finds a term present only in an Entry's context", () => {
    const { result } = renderHook(() => useFilters(ITEMS));

    act(() => result.current.setQuery("dispatchToRoute"));

    expect(result.current.filtered).toHaveLength(1);
    expect(
      isEntry(result.current.filtered[0]) && result.current.filtered[0].message,
    ).toBe("Unhandled exception");
  });
});

describe("useFilters — Level threshold", () => {
  it("uses severity rank rather than string order", () => {
    const { result } = renderHook(() => useFilters(ITEMS));

    act(() => result.current.setMinLevel("info"));
    expect(result.current.filtered).toHaveLength(2);

    act(() => result.current.setMinLevel("error"));
    expect(result.current.filtered).toHaveLength(1);
  });
});
