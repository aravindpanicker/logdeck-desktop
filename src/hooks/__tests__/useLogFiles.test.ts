/**
 * The Target picker's directory listing (D5).
 *
 * The picker itself is not under test — D10 puts no component there — but the
 * hook behind it decides three things a reader would be misled by: that the
 * listing is read when `refresh` is called and not on mount, that a failed read
 * is surfaced rather than shown as an empty directory (LESSONS 2), and that a
 * reply which is not a listing is refused before it reaches the menu's `map`.
 */

import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { createFakeTransport } from "./fakeTransport";
import { PROJECT } from "./fixtures";
import { useLogFiles } from "../useLogFiles";
import { toProjectId, type LogFile } from "../../lib/types";

afterEach(cleanup);

const LARAVEL_LOG: LogFile = {
  name: "laravel.log",
  bytes: 2048,
  modified: 1_800_000_000,
};

describe("useLogFiles", () => {
  it("reads nothing until the menu asks for it", async () => {
    const transport = createFakeTransport();
    transport.reply("list_log_files", [LARAVEL_LOG]);

    const { result } = renderHook(() => useLogFiles(PROJECT, transport));
    await act(async () => {});

    expect(transport.calls).toHaveLength(0);
    expect(result.current.files).toEqual([]);

    await act(async () => {
      await result.current.refresh();
    });

    expect(transport.calls).toEqual([
      { command: "list_log_files", args: { projectId: PROJECT } },
    ]);
    expect(result.current.files).toEqual([LARAVEL_LOG]);
    expect(result.current.error).toBeNull();
  });

  it("surfaces a failed read instead of an empty directory", async () => {
    const transport = createFakeTransport();
    transport.fail("list_log_files", "storage/logs is gone");

    const { result } = renderHook(() => useLogFiles(PROJECT, transport));
    await act(async () => {
      await result.current.refresh();
    });

    // The bare string a Rust `Err(String)` becomes is the message worth showing.
    expect(result.current.error).toBe("storage/logs is gone");
    expect(result.current.files).toEqual([]);
    expect(result.current.isLoading).toBe(false);
  });

  it("falls back to a message of its own when the failure carries none", async () => {
    const transport = createFakeTransport();
    transport.fail("list_log_files", { code: 13 });

    const { result } = renderHook(() => useLogFiles(PROJECT, transport));
    await act(async () => {
      await result.current.refresh();
    });

    expect(result.current.error).toBe("The log directory could not be read");
  });

  it("refuses a reply that is not a listing rather than rendering it", async () => {
    const transport = createFakeTransport();
    // A shape mismatch, not a nullish reply: the fake's `?? []` would hide a
    // `null` behind the very default this test exists to stop relying on.
    transport.reply("list_log_files", { files: [LARAVEL_LOG] });

    const { result } = renderHook(() => useLogFiles(PROJECT, transport));
    await act(async () => {
      await result.current.refresh();
    });

    // `files.map` in the picker would have thrown during render — a blank
    // window instead of a message.
    expect(result.current.files).toEqual([]);
    expect(result.current.error).toBe("list_log_files did not return a listing");
  });

  it("drops an item that is not a LogFile and keeps the rest", async () => {
    const transport = createFakeTransport();
    transport.reply("list_log_files", [LARAVEL_LOG, { name: "half.log" }]);

    const { result } = renderHook(() => useLogFiles(PROJECT, transport));
    await act(async () => {
      await result.current.refresh();
    });

    expect(result.current.files).toEqual([LARAVEL_LOG]);
    expect(result.current.error).toBeNull();
  });

  it("clears the listing when the Project changes", async () => {
    const transport = createFakeTransport();
    transport.reply("list_log_files", [LARAVEL_LOG]);

    const { result, rerender } = renderHook(
      ({ id }) => useLogFiles(id, transport),
      { initialProps: { id: PROJECT as typeof PROJECT | null } },
    );
    await act(async () => {
      await result.current.refresh();
    });
    expect(result.current.files).toHaveLength(1);

    rerender({ id: toProjectId("/Users/dev/other") });

    // Another Project's directory, never shown beside this Project's name.
    await waitFor(() => expect(result.current.files).toEqual([]));
  });

  it("does not read a directory when no Project is selected", async () => {
    const transport = createFakeTransport();
    const { result } = renderHook(() => useLogFiles(null, transport));

    await act(async () => {
      await result.current.refresh();
    });
    expect(transport.calls).toHaveLength(0);
  });
});
