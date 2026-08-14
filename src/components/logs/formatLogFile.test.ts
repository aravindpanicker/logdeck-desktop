import { describe, expect, it } from "vitest";

import { describeLogFile, formatAge, formatBytes } from "./formatLogFile";

describe("formatBytes", () => {
  it("renders whole bytes without a decimal", () => {
    expect(formatBytes(0)).toBe("0 B");
    expect(formatBytes(1)).toBe("1 B");
    expect(formatBytes(1023)).toBe("1023 B");
  });

  it("steps up a unit at 1024 and keeps one decimal while it carries meaning", () => {
    expect(formatBytes(1024)).toBe("1.0 KB");
    expect(formatBytes(1536)).toBe("1.5 KB");
    expect(formatBytes(1024 * 1024)).toBe("1.0 MB");
    expect(formatBytes(1024 * 1024 * 1024)).toBe("1.0 GB");
  });

  it("drops the decimal once the magnitude carries the information", () => {
    expect(formatBytes(1024 * 12)).toBe("12 KB");
    expect(formatBytes(1024 * 1024 * 40)).toBe("40 MB");
  });

  it("treats a nonsense size as empty rather than throwing", () => {
    expect(formatBytes(-1)).toBe("0 B");
    expect(formatBytes(Number.NaN)).toBe("0 B");
  });
});

describe("formatAge", () => {
  const NOW = 1_800_000_000;

  it("calls the last minute 'just now' so the row does not tick while read", () => {
    expect(formatAge(NOW, NOW)).toBe("just now");
    expect(formatAge(NOW - 44, NOW)).toBe("just now");
  });

  it("counts up through minutes, hours, days and weeks", () => {
    expect(formatAge(NOW - 60 * 5, NOW)).toBe("5m ago");
    expect(formatAge(NOW - 3600 * 3, NOW)).toBe("3h ago");
    expect(formatAge(NOW - 86400 * 2, NOW)).toBe("2d ago");
    expect(formatAge(NOW - 86400 * 21, NOW)).toBe("3w ago");
  });

  it("reads a future mtime as 'just now' rather than as a negative age", () => {
    // A container writing on a skewed clock is the ordinary cause of this.
    expect(formatAge(NOW + 3600, NOW)).toBe("just now");
  });

  it("does not invent an age it cannot compute", () => {
    expect(formatAge(Number.NaN, NOW)).toBe("unknown");
  });
});

describe("describeLogFile", () => {
  it("joins the size and the age into the one line the picker shows", () => {
    const now = 1_800_000_000;
    expect(
      describeLogFile({ bytes: 1024 * 12, modified: now - 3600 }, now),
    ).toBe("12 KB · 1h ago");
  });
});
