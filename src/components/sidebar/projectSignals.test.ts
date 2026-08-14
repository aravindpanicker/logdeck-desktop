/**
 * The sidebar's pure interpretation layer.
 *
 * Not a component test (D10 puts those out of scope) — these pin the two
 * behaviours that would otherwise only fail live: a `project:activity` payload
 * whose shape surprises us must not crash the listener, and an unhealthy
 * **Project** must produce a sentence a human can act on (D4).
 */

import { describe, expect, test } from "vitest";

import {
  describeHealth,
  formatActivityCount,
  OFFLINE_FALLBACK_REASON,
  readActivity,
  readProjects,
  readStatus,
  shouldShowActivityBadge,
} from "./projectSignals";

describe("readActivity", () => {
  test("reads the count and highest Level from a well-formed payload", () => {
    const signal = readActivity({
      projectId: "/work/api",
      total: 12,
      counts: { error: 2, info: 10 },
      maxLevel: "error",
    });

    expect(signal).toEqual({
      projectId: "/work/api",
      activity: { total: 12, maxLevel: "error" },
    });
  });

  test("falls back to summing counts when total is absent", () => {
    const signal = readActivity({
      projectId: "/work/api",
      counts: { warning: 3, info: 4 },
      maxLevel: "warning",
    });

    expect(signal?.activity.total).toBe(7);
  });

  test("a Level it does not recognise becomes unknown rather than being invented", () => {
    // "unknown" ranks below "debug", so an unreadable payload can never inflate
    // the Activity rollup (D8).
    const signal = readActivity({ projectId: "/work/api", total: 1, maxLevel: "PANIC" });

    expect(signal?.activity.maxLevel).toBe("unknown");
  });

  test.each([
    ["not an object", 42],
    ["null", null],
    ["no projectId", { total: 1, maxLevel: "info" }],
    ["an empty projectId", { projectId: "", total: 1 }],
    ["no total and no counts", { projectId: "/work/api", maxLevel: "info" }],
  ])("drops a payload with %s", (_case, payload) => {
    expect(readActivity(payload)).toBeNull();
  });
});

describe("readStatus", () => {
  test("carries the reason a Project went offline", () => {
    expect(readStatus({ projectId: "/work/api", state: "offline", reason: "path vanished" })).toEqual(
      { projectId: "/work/api", offlineReason: "path vanished" },
    );
  });

  test("supplies a reason when the payload omits one", () => {
    expect(
      readStatus({ projectId: "/work/api", state: "offline" })?.offlineReason,
    ).toBe(OFFLINE_FALLBACK_REASON);
  });

  test("an online status clears the reason, so a Project recovers on its own", () => {
    // D9: recovery needs no user action.
    expect(readStatus({ projectId: "/work/api", state: "online" })?.offlineReason).toBeNull();
  });

  test("a state this build does not know reads as online rather than dimming forever", () => {
    expect(readStatus({ projectId: "/work/api", state: "reattaching" })?.offlineReason).toBeNull();
  });

  test("drops a payload with no state", () => {
    expect(readStatus({ projectId: "/work/api" })).toBeNull();
  });
});

describe("describeHealth", () => {
  test("a healthy Project has nothing to say", () => {
    expect(describeHealth("ok")).toBeNull();
  });

  test.each([
    ["noLogsDir" as const, "no storage/logs found"],
    ["notLaravel" as const, "not a Laravel project"],
  ])("%s reads as a sentence, not a variant name", (health, expected) => {
    expect(describeHealth(health)).toBe(expected);
  });

  test("an unavailable Project shows the reason it carries", () => {
    expect(describeHealth({ unavailable: "cannot resolve `/gone`" })).toBe(
      "cannot resolve `/gone`",
    );
  });
});

describe("readProjects", () => {
  test("reads a well-formed registry", () => {
    expect(
      readProjects([
        { id: "/work/api", label: "api", path: "/work/api", health: "ok" },
        {
          id: "/work/web",
          label: "web",
          path: "/work/web",
          health: { unavailable: "cannot resolve `/work/web`" },
        },
      ]),
    ).toEqual([
      { id: "/work/api", label: "api", path: "/work/api", health: "ok" },
      {
        id: "/work/web",
        label: "web",
        path: "/work/web",
        health: { unavailable: "cannot resolve `/work/web`" },
      },
    ]);
  });

  test("a response that is not a list is reported, not read as an empty registry", () => {
    // An empty registry is a state the sidebar has copy for; a broken contract
    // is not, and must not masquerade as one.
    expect(readProjects({ projects: [] })).toBeNull();
    expect(readProjects(undefined)).toBeNull();
    expect(readProjects([])).toEqual([]);
  });

  test("drops a record with no id, because remove and select are keyed by it", () => {
    expect(readProjects([{ label: "api", path: "/work/api", health: "ok" }])).toEqual([]);
  });

  test("a Project the user registered survives a missing label or path (D4)", () => {
    expect(readProjects([{ id: "/work/api" }])).toEqual([
      { id: "/work/api", label: "/work/api", path: "/work/api", health: "ok" },
    ]);
  });

  test("a Health variant this build does not know reads as ok rather than dimming forever", () => {
    expect(readProjects([{ id: "/work/api", label: "api", path: "/work/api", health: "quarantined" }])).toEqual([
      { id: "/work/api", label: "api", path: "/work/api", health: "ok" },
    ]);
  });
});

describe("shouldShowActivityBadge", () => {
  test("shows the badge for an unselected Project with unread Entries", () => {
    expect(shouldShowActivityBadge(false, { total: 3, maxLevel: "error" })).toBe(true);
  });

  test("selecting a Project clears its badge (D8)", () => {
    expect(shouldShowActivityBadge(true, { total: 3, maxLevel: "error" })).toBe(false);
  });

  test("nothing unread means no badge, not a zero", () => {
    expect(shouldShowActivityBadge(false, { total: 0, maxLevel: "unknown" })).toBe(false);
    expect(shouldShowActivityBadge(false, undefined)).toBe(false);
  });
});

describe("formatActivityCount", () => {
  test("caps the badge so a busy Project cannot widen the row", () => {
    expect(formatActivityCount(4)).toBe("4");
    expect(formatActivityCount(999)).toBe("999");
    expect(formatActivityCount(1000)).toBe("999+");
  });
});
