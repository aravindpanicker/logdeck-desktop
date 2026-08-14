import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { createFakeTransport, type FakeTransport } from "./fakeTransport";
import { useLogStream } from "../useLogStream";
import { isBreak, isEntry, toProjectId } from "../../lib/types";
import {
  breakItem,
  entryItem,
  makeBreak,
  makeEntries,
  makeEntry,
  PROJECT,
} from "./fixtures";

afterEach(cleanup);

function mount(transport: FakeTransport, projectId = PROJECT) {
  return renderHook(({ id }) => useLogStream(id, transport), {
    initialProps: { id: projectId as typeof PROJECT | null },
  });
}

describe("useLogStream — opening window", () => {
  it("loads the window returned by select_project", async () => {
    const transport = createFakeTransport();
    transport.reply("select_project", makeEntries(3).map(entryItem));

    const { result } = mount(transport);
    await waitFor(() => expect(result.current.items).toHaveLength(3));

    expect(transport.calls).toContainEqual({
      command: "select_project",
      args: { projectId: PROJECT },
    });
  });

  it("surfaces a failed select_project instead of swallowing it", async () => {
    const transport = createFakeTransport();
    transport.fail("select_project", new Error("project is unavailable"));

    const { result } = mount(transport);

    await waitFor(() => expect(result.current.error).toBe("project is unavailable"));
    expect(result.current.items).toHaveLength(0);
  });

  it("does not select anything when no Project is selected", async () => {
    const transport = createFakeTransport();
    renderHook(() => useLogStream(null, transport));

    await act(async () => {});
    expect(transport.calls).toHaveLength(0);
  });
});

describe("useLogStream — log:entry upsert (D2)", () => {
  it("appends an unseen Entry and replaces a known id in place", async () => {
    const transport = createFakeTransport();
    const { result } = mount(transport);
    await waitFor(() => expect(transport.listenerCount("log:entry")).toBe(1));

    act(() => {
      transport.emit("log:entry", makeEntry({ offset: 0, message: "first" }));
      transport.emit("log:entry", makeEntry({ offset: 100, message: "torn" }));
      transport.emit("log:entry", makeEntry({ offset: 300, message: "third" }));
    });
    expect(result.current.items).toHaveLength(3);

    act(() => {
      transport.emit(
        "log:entry",
        makeEntry({ offset: 100, message: "torn", context: "#0 frame()" }),
      );
    });

    const items = result.current.items;
    expect(items).toHaveLength(3);
    expect(isEntry(items[1]) && items[1].context).toBe("#0 frame()");
    expect(isEntry(items[2]) && items[2].message).toBe("third");
  });

  it("ignores an Entry belonging to another Project", async () => {
    const transport = createFakeTransport();
    const { result } = mount(transport);
    await waitFor(() => expect(transport.listenerCount("log:entry")).toBe(1));

    act(() => {
      transport.emit(
        "log:entry",
        makeEntry({ offset: 0, projectId: toProjectId("/Users/dev/other") }),
      );
    });

    expect(result.current.items).toHaveLength(0);
  });
});

describe("useLogStream — log:entries batching", () => {
  it("upserts a whole batch and republishes exactly once", async () => {
    const transport = createFakeTransport();
    let renders = 0;
    const { result } = renderHook(() => {
      renders += 1;
      return useLogStream(PROJECT, transport);
    });
    await waitFor(() => expect(transport.listenerCount("log:entries")).toBe(1));
    await waitFor(() => expect(result.current.isLoading).toBe(false));
    await act(async () => {});

    const settled = renders;
    act(() => {
      transport.emit(
        "log:entries",
        Array.from({ length: 500 }, (_unused, position) =>
          makeEntry({ offset: position * 100, message: `boom ${position}` }),
        ),
      );
    });

    expect(result.current.items).toHaveLength(500);
    // The regression this event exists for: 500 separate `log:entry` events
    // would be 500 snapshot copies, 500 filter passes and 500 commits.
    expect(renders - settled).toBe(1);
  });

  it("keeps a batch belonging to another Project out of the record", async () => {
    const transport = createFakeTransport();
    const { result } = mount(transport);
    await waitFor(() => expect(transport.listenerCount("log:entries")).toBe(1));

    act(() => {
      transport.emit("log:entries", [
        makeEntry({ offset: 0, projectId: toProjectId("/Users/dev/other") }),
        makeEntry({ offset: 100, message: "mine" }),
      ]);
    });

    const items = result.current.items;
    expect(items).toHaveLength(1);
    expect(isEntry(items[0]) && items[0].message).toBe("mine");
  });

  it("revises in place through a batch, exactly as a single event does (D2)", async () => {
    const transport = createFakeTransport();
    const { result } = mount(transport);
    await waitFor(() => expect(transport.listenerCount("log:entries")).toBe(1));

    act(() => {
      transport.emit("log:entries", [
        makeEntry({ offset: 0, message: "first" }),
        makeEntry({ offset: 100, message: "torn" }),
      ]);
    });
    act(() => {
      transport.emit("log:entries", [
        makeEntry({ offset: 100, message: "torn", context: "#0 frame()" }),
      ]);
    });

    const items = result.current.items;
    expect(items).toHaveLength(2);
    expect(isEntry(items[1]) && items[1].context).toBe("#0 frame()");
  });
});

describe("useLogStream — log:break (D3, ADR 0001)", () => {
  it("appends the marker and keeps every Entry above it", async () => {
    const transport = createFakeTransport();
    const opening = makeEntries(4).map(entryItem);
    transport.reply("select_project", opening);

    const { result } = mount(transport);
    await waitFor(() => expect(result.current.items).toHaveLength(4));

    act(() => {
      transport.emit("log:break", makeBreak(9999, "cleared"));
    });

    const items = result.current.items;
    expect(items).toHaveLength(5);
    expect(items.slice(0, 4)).toEqual(opening);
    expect(isBreak(items[4]) && items[4].kind).toBe("cleared");
  });

  it("ignores a Break belonging to another Project", async () => {
    const transport = createFakeTransport();
    const { result } = mount(transport);
    await waitFor(() => expect(transport.listenerCount("log:break")).toBe(1));

    act(() => {
      const foreign = makeBreak(9999, "rotated");
      transport.emit("log:break", {
        ...foreign,
        projectId: toProjectId("/Users/dev/other"),
      });
    });

    expect(result.current.items).toHaveLength(0);
  });
});

describe("useLogStream — load_earlier", () => {
  it("prepends the earlier page above what is already held", async () => {
    const transport = createFakeTransport();
    transport.reply("select_project", [
      entryItem(makeEntry({ offset: 500, message: "held" })),
    ]);
    transport.reply("load_earlier", [
      breakItem(makeBreak(100, "rotated")),
      entryItem(makeEntry({ offset: 200, message: "older" })),
    ]);

    const { result } = mount(transport);
    await waitFor(() => expect(result.current.items).toHaveLength(1));

    await act(async () => {
      await result.current.loadEarlier();
    });

    const items = result.current.items;
    expect(items).toHaveLength(3);
    expect(isBreak(items[0])).toBe(true);
    expect(isEntry(items[1]) && items[1].message).toBe("older");
    expect(isEntry(items[2]) && items[2].message).toBe("held");
    // The id, not the offset: after a Break the oldest Entry held belongs to
    // the file *before* the Break, and only the id carries which file and which
    // id generation that was (§3).
    expect(transport.calls).toContainEqual({
      command: "load_earlier",
      args: { projectId: PROJECT, beforeId: "laravel.log:500" },
    });
  });

  it("pages from the oldest held Entry's own file after a Break", async () => {
    const transport = createFakeTransport();
    // A rotated record: yesterday's Entries, the Break, then today's.
    transport.reply("select_project", [
      entryItem(
        makeEntry({
          file: "laravel-2026-08-13.log",
          offset: 900,
          message: "yesterday",
        }),
      ),
      breakItem(makeBreak(0, "rotated", "laravel-2026-08-14.log")),
      entryItem(
        makeEntry({
          file: "laravel-2026-08-14.log",
          offset: 0,
          message: "today",
        }),
      ),
    ]);
    transport.reply("load_earlier", []);

    const { result } = mount(transport);
    await waitFor(() => expect(result.current.items).toHaveLength(3));

    await act(async () => {
      await result.current.loadEarlier();
    });

    expect(transport.calls).toContainEqual({
      command: "load_earlier",
      args: { projectId: PROJECT, beforeId: "laravel-2026-08-13.log:900" },
    });
  });

  it("surfaces a failed load_earlier and releases the loading flag", async () => {
    const transport = createFakeTransport();
    transport.reply("select_project", [
      entryItem(makeEntry({ offset: 500, message: "held" })),
    ]);
    transport.fail("load_earlier", new Error("the log file is unavailable"));

    const { result } = mount(transport);
    await waitFor(() => expect(result.current.items).toHaveLength(1));

    await act(async () => {
      await result.current.loadEarlier();
    });

    expect(result.current.error).toBe("the log file is unavailable");
    // Left stuck true, the "load earlier" affordance would never come back.
    expect(result.current.isLoadingEarlier).toBe(false);
    expect(result.current.items).toHaveLength(1);
  });

  it("raises isLoadingEarlier only while the request is in flight", async () => {
    const transport = createFakeTransport();
    transport.reply("select_project", [
      entryItem(makeEntry({ offset: 500, message: "held" })),
    ]);

    const { result } = mount(transport);
    await waitFor(() => expect(result.current.items).toHaveLength(1));
    expect(result.current.isLoadingEarlier).toBe(false);

    transport.defer("load_earlier");
    let pending!: Promise<void>;
    act(() => {
      pending = result.current.loadEarlier();
    });
    await waitFor(() => expect(result.current.isLoadingEarlier).toBe(true));

    await act(async () => {
      transport.settle("load_earlier", []);
      await pending;
    });
    expect(result.current.isLoadingEarlier).toBe(false);
  });

  it("clears a stale error once a later load_earlier succeeds", async () => {
    const transport = createFakeTransport();
    transport.reply("select_project", [
      entryItem(makeEntry({ offset: 500, message: "held" })),
    ]);
    transport.fail("load_earlier", new Error("transient bridge failure"));

    const { result } = mount(transport);
    await waitFor(() => expect(result.current.items).toHaveLength(1));

    await act(async () => {
      await result.current.loadEarlier();
    });
    expect(result.current.error).toBe("transient bridge failure");

    transport.reply("load_earlier", [
      entryItem(makeEntry({ offset: 200, message: "older" })),
    ]);
    await act(async () => {
      await result.current.loadEarlier();
    });

    expect(result.current.items).toHaveLength(2);
    expect(result.current.error).toBeNull();
  });

  it("does not page when nothing held carries an offset to page from", async () => {
    const transport = createFakeTransport();
    // An opening window that is only a leading Break — legitimate right after a
    // truncation with nothing readable above it.
    transport.reply("select_project", [breakItem(makeBreak(0, "cleared"))]);

    const { result } = mount(transport);
    await waitFor(() => expect(result.current.items).toHaveLength(1));

    await act(async () => {
      await result.current.loadEarlier();
    });

    expect(transport.calls.filter((call) => call.command === "load_earlier")).toEqual([]);
    expect(result.current.error).toBeNull();
  });

  it("does not issue a second request while one is in flight", async () => {
    const transport = createFakeTransport();
    transport.reply("select_project", [
      entryItem(makeEntry({ offset: 500, message: "held" })),
    ]);

    const { result } = mount(transport);
    await waitFor(() => expect(result.current.items).toHaveLength(1));

    transport.defer("load_earlier");
    let first!: Promise<void>;
    let second!: Promise<void>;
    act(() => {
      first = result.current.loadEarlier();
      second = result.current.loadEarlier();
    });

    expect(
      transport.calls.filter((call) => call.command === "load_earlier"),
    ).toHaveLength(1);

    await act(async () => {
      transport.settle("load_earlier", []);
      await Promise.all([first, second]);
    });
  });

  it("drops an earlier page whose Project is no longer selected", async () => {
    const transport = createFakeTransport();
    transport.reply("select_project", [
      entryItem(makeEntry({ offset: 500, message: "held by A" })),
    ]);

    const { result, rerender } = mount(transport);
    await waitFor(() => expect(result.current.items).toHaveLength(1));

    transport.defer("load_earlier");
    let pending!: Promise<void>;
    act(() => {
      pending = result.current.loadEarlier();
    });

    const other = toProjectId("/Users/dev/other");
    transport.reply("select_project", []);
    rerender({ id: other });
    await waitFor(() => expect(result.current.items).toHaveLength(0));

    await act(async () => {
      transport.settle("load_earlier", [
        entryItem(makeEntry({ offset: 200, message: "older, from A" })),
      ]);
      await pending;
    });

    // A's page must not land in B's Session Record.
    expect(result.current.items).toHaveLength(0);
  });
});

describe("useLogStream — cleanup", () => {
  it("still detaches the listener that succeeded when the other subscription fails", async () => {
    const transport = createFakeTransport();
    transport.failListen("log:break", new Error("the bridge dropped out"));

    const { result, unmount } = mount(transport);

    await waitFor(() => expect(result.current.error).toBe("the bridge dropped out"));
    expect(transport.listenerCount("log:entry")).toBe(1);

    unmount();

    // Without the Unlisten being captured, this listener would leak forever.
    expect(transport.listenerCount("log:entry")).toBe(0);
  });


  it("unlistens on unmount and drops events that arrive afterwards", async () => {
    const transport = createFakeTransport();
    const { result, unmount } = mount(transport);
    await waitFor(() => expect(transport.listenerCount("log:entry")).toBe(1));

    act(() => {
      transport.emit("log:entry", makeEntry({ offset: 0 }));
    });
    expect(result.current.items).toHaveLength(1);

    unmount();

    expect(transport.listenerCount("log:entry")).toBe(0);
    expect(transport.listenerCount("log:break")).toBe(0);
    transport.emit("log:entry", makeEntry({ offset: 100 }));
  });

  it("starts a clean Session Record when the selected Project changes", async () => {
    const transport = createFakeTransport();
    transport.reply("select_project", [entryItem(makeEntry({ offset: 0 }))]);

    const { result, rerender } = mount(transport);
    await waitFor(() => expect(result.current.items).toHaveLength(1));

    const other = toProjectId("/Users/dev/other");
    transport.reply("select_project", []);
    rerender({ id: other });

    await waitFor(() => expect(result.current.items).toHaveLength(0));
    expect(transport.listenerCount("log:entry")).toBe(1);
    expect(transport.calls).toContainEqual({
      command: "select_project",
      args: { projectId: other },
    });
  });
});

describe("useLogStream — Target (D5)", () => {
  it("starts on Latest and follows the newest file", async () => {
    const transport = createFakeTransport();
    const { result } = mount(transport);
    await waitFor(() => expect(result.current.isLoading).toBe(false));

    expect(result.current.target).toBe("latest");
    expect(
      transport.calls.filter((call) => call.command === "set_target"),
    ).toEqual([]);
  });

  it("pins a file and REPLACES the record with the window it returns", async () => {
    const transport = createFakeTransport();
    transport.reply("select_project", [
      entryItem(makeEntry({ offset: 0, message: "today one" })),
      entryItem(makeEntry({ offset: 100, message: "today two" })),
    ]);
    transport.reply("set_target", [
      entryItem(
        makeEntry({
          file: "laravel-2026-08-13.log",
          offset: 0,
          message: "yesterday",
        }),
      ),
    ]);

    const { result } = mount(transport);
    await waitFor(() => expect(result.current.items).toHaveLength(2));

    await act(async () => {
      await result.current.setTarget({ file: "laravel-2026-08-13.log" });
    });

    // Not prepended: yesterday's Entries above today's would read backwards in
    // time, and `load_earlier` would then page from the wrong file.
    const items = result.current.items;
    expect(items).toHaveLength(1);
    expect(isEntry(items[0]) && items[0].message).toBe("yesterday");
    expect(result.current.target).toEqual({ file: "laravel-2026-08-13.log" });
    expect(transport.calls).toContainEqual({
      command: "set_target",
      args: { projectId: PROJECT, target: { file: "laravel-2026-08-13.log" } },
    });
  });

  it("keeps delivering Entries into a pinned Target — pinning is not stopping", async () => {
    const transport = createFakeTransport();
    // Arranged explicitly rather than leaning on the fake's default for an
    // unconfigured command: this test is about the pin, and it must fail for
    // that reason or not at all.
    transport.reply("select_project", []);
    transport.reply("set_target", [
      entryItem(
        makeEntry({ file: "laravel.log", offset: 0, message: "already there" }),
      ),
    ]);

    const { result } = mount(transport);
    await waitFor(() => expect(transport.listenerCount("log:entry")).toBe(1));

    await act(async () => {
      await result.current.setTarget({ file: "laravel.log" });
    });
    expect(result.current.items).toHaveLength(1);

    // The watcher still polls the pinned file every tick.
    act(() => {
      transport.emit("log:entry", makeEntry({ offset: 400, message: "live" }));
    });

    const items = result.current.items;
    expect(items).toHaveLength(2);
    expect(isEntry(items[1]) && items[1].message).toBe("live");
  });

  it("returns to Latest by replacing the record with the following window", async () => {
    const transport = createFakeTransport();
    transport.reply("set_target", [
      entryItem(
        makeEntry({
          file: "laravel-2026-08-13.log",
          offset: 0,
          message: "yesterday",
        }),
      ),
    ]);

    const { result } = mount(transport);
    await waitFor(() => expect(result.current.isLoading).toBe(false));

    await act(async () => {
      await result.current.setTarget({ file: "laravel-2026-08-13.log" });
    });
    expect(result.current.target).toEqual({ file: "laravel-2026-08-13.log" });

    transport.reply("set_target", [
      entryItem(makeEntry({ offset: 900, message: "today" })),
    ]);
    await act(async () => {
      await result.current.setTarget("latest");
    });

    expect(result.current.target).toBe("latest");
    const items = result.current.items;
    expect(items).toHaveLength(1);
    expect(isEntry(items[0]) && items[0].message).toBe("today");
    expect(transport.calls).toContainEqual({
      command: "set_target",
      args: { projectId: PROJECT, target: "latest" },
    });
  });

  it("resets the Target to Latest when the selected Project changes", async () => {
    const transport = createFakeTransport();
    transport.reply("set_target", [
      entryItem(makeEntry({ file: "laravel-2026-08-13.log", offset: 0 })),
    ]);

    const { result, rerender } = mount(transport);
    await waitFor(() => expect(result.current.isLoading).toBe(false));

    await act(async () => {
      await result.current.setTarget({ file: "laravel-2026-08-13.log" });
    });
    expect(result.current.target).toEqual({ file: "laravel-2026-08-13.log" });

    // A pin belongs to the Project it was made in.
    transport.reply("select_project", []);
    rerender({ id: toProjectId("/Users/dev/other") });

    await waitFor(() => expect(result.current.target).toBe("latest"));
    expect(result.current.items).toHaveLength(0);
  });

  it("surfaces a failed pin and keeps the Target it is actually reading", async () => {
    const transport = createFakeTransport();
    transport.reply("select_project", [
      entryItem(makeEntry({ offset: 0, message: "held" })),
    ]);
    transport.fail("set_target", new Error("laravel-2026-08-01.log is gone"));

    const { result } = mount(transport);
    await waitFor(() => expect(result.current.items).toHaveLength(1));

    await act(async () => {
      await result.current.setTarget({ file: "laravel-2026-08-01.log" });
    });

    expect(result.current.error).toBe("laravel-2026-08-01.log is gone");
    expect(result.current.target).toBe("latest");
    // A failed pin must not take the record with it.
    expect(result.current.items).toHaveLength(1);
    expect(result.current.isRetargeting).toBe(false);
  });

  it("raises isRetargeting only while the pin is in flight", async () => {
    const transport = createFakeTransport();
    const { result } = mount(transport);
    await waitFor(() => expect(result.current.isLoading).toBe(false));
    expect(result.current.isRetargeting).toBe(false);

    transport.defer("set_target");
    let pending!: Promise<void>;
    act(() => {
      pending = result.current.setTarget({ file: "laravel.log" });
    });
    await waitFor(() => expect(result.current.isRetargeting).toBe(true));

    await act(async () => {
      transport.settle("set_target", []);
      await pending;
    });
    expect(result.current.isRetargeting).toBe(false);
  });

  it("drops an earlier page that was requested before the Target changed", async () => {
    const transport = createFakeTransport();
    transport.reply("select_project", [
      entryItem(makeEntry({ offset: 500, message: "today" })),
    ]);
    transport.reply("set_target", [
      entryItem(
        makeEntry({
          file: "laravel-2026-08-13.log",
          offset: 0,
          message: "yesterday",
        }),
      ),
    ]);

    const { result } = mount(transport);
    await waitFor(() => expect(result.current.items).toHaveLength(1));

    transport.defer("load_earlier");
    let paging!: Promise<void>;
    act(() => {
      paging = result.current.loadEarlier();
    });

    await act(async () => {
      await result.current.setTarget({ file: "laravel-2026-08-13.log" });
    });

    await act(async () => {
      transport.settle("load_earlier", [
        entryItem(makeEntry({ offset: 100, message: "older today" })),
      ]);
      await paging;
    });

    // The page belongs to a record the reader has navigated away from.
    const items = result.current.items;
    expect(items).toHaveLength(1);
    expect(isEntry(items[0]) && items[0].message).toBe("yesterday");
  });
});

/**
 * `invoke<StreamItem[]>` is a claim about the Rust side, not a check on it. The
 * `log:entries` listener already refuses a payload it cannot trust; these pin
 * the same posture for the three commands, whose replies go straight into the
 * buffer.
 */
describe("useLogStream — a reply that is not a window", () => {
  it("surfaces a select_project reply that is not an array", async () => {
    const transport = createFakeTransport();
    transport.reply("select_project", { items: [] });

    const { result } = mount(transport);

    await waitFor(() =>
      expect(result.current.error).toBe(
        "select_project did not return a window of log items",
      ),
    );
    expect(result.current.items).toHaveLength(0);
    expect(result.current.isLoading).toBe(false);
  });

  it("fails a pin whose reply is not an array without emptying the record", async () => {
    const transport = createFakeTransport();
    transport.reply("select_project", [
      entryItem(makeEntry({ offset: 0, message: "held" })),
    ]);
    transport.reply("set_target", { items: [] });

    const { result } = mount(transport);
    await waitFor(() => expect(result.current.items).toHaveLength(1));

    await act(async () => {
      await result.current.setTarget({ file: "laravel-2026-08-13.log" });
    });

    expect(result.current.error).toBe(
      "set_target did not return a window of log items",
    );
    // Narrowed before `clear()`: the reader keeps what they were reading, and
    // the label keeps naming it.
    expect(result.current.items).toHaveLength(1);
    expect(isEntry(result.current.items[0]) && result.current.items[0].message)
      .toBe("held");
    expect(result.current.target).toBe("latest");
    expect(result.current.isRetargeting).toBe(false);
  });

  it("surfaces a load_earlier reply that is not an array and keeps the page held", async () => {
    const transport = createFakeTransport();
    transport.reply("select_project", [
      entryItem(makeEntry({ offset: 500, message: "held" })),
    ]);
    transport.reply("load_earlier", { items: [] });

    const { result } = mount(transport);
    await waitFor(() => expect(result.current.items).toHaveLength(1));

    await act(async () => {
      await result.current.loadEarlier();
    });

    expect(result.current.error).toBe(
      "load_earlier did not return a window of log items",
    );
    expect(result.current.items).toHaveLength(1);
    expect(result.current.isLoadingEarlier).toBe(false);
  });

  it("drops an item that is neither an Entry nor a Break and keeps the rest", async () => {
    const transport = createFakeTransport();
    transport.reply("select_project", [
      entryItem(makeEntry({ offset: 0, message: "real" })),
      { type: "something-else" },
      null,
    ]);

    const { result } = mount(transport);
    await waitFor(() => expect(result.current.items).toHaveLength(1));

    // An item with no id would corrupt the buffer's upsert for every Entry
    // after it, so it never reaches the buffer at all.
    expect(isEntry(result.current.items[0]) && result.current.items[0].message)
      .toBe("real");
    expect(result.current.error).toBeNull();
  });
});
