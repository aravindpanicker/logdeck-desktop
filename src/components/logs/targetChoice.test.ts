import { describe, expect, it } from "vitest";

import { isCurrentTarget, pinnedFile } from "./targetChoice";
import type { LogFile } from "../../lib/types";

const file = (name: string): LogFile => ({
  name,
  bytes: 1024,
  modified: 1_800_000_000,
});

describe("pinnedFile", () => {
  it("names no file while following the newest one", () => {
    expect(pinnedFile("latest")).toBeNull();
  });

  it("reads the file name out of the pinned arm", () => {
    expect(pinnedFile({ file: "laravel-2026-08-13.log" })).toBe(
      "laravel-2026-08-13.log",
    );
  });
});

describe("isCurrentTarget", () => {
  it("marks the Latest row current exactly when nothing is pinned", () => {
    expect(isCurrentTarget("latest", null)).toBe(true);
    expect(isCurrentTarget({ file: "laravel.log" }, null)).toBe(false);
  });

  it("marks the pinned file current and no other file", () => {
    const target = { file: "laravel-2026-08-13.log" } as const;
    expect(isCurrentTarget(target, file("laravel-2026-08-13.log"))).toBe(true);
    expect(isCurrentTarget(target, file("laravel-2026-08-12.log"))).toBe(false);
  });

  it("marks no file row current while following Latest", () => {
    // The Latest row is the only current one; a file row must not also claim it,
    // or `aria-checked` would be true twice in one radio group.
    expect(isCurrentTarget("latest", file("laravel.log"))).toBe(false);
  });
});
