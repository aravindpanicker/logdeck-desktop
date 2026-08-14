/**
 * What a **Project** the user is not reading shows about what it has been
 * writing (D8, [ADR 0002](../../../docs/adr/0002-watch-all-projects-retain-only-the-selected-one.md)).
 *
 * The badge answers two questions and no others: how much, and how bad. The
 * failure modes worth a test are all things a reader would act on wrongly — a
 * count on a Project they are already reading, a badge shouting `0`, or a
 * severity that is the most *recent* thing written rather than the *worst*.
 *
 * The payload is read through `readActivity` here rather than hand-built, so
 * what is under test is the whole path from a `project:activity` event to the
 * glyph, which is where the "highest, not latest" question actually lives.
 *
 * See `ProjectRow.test.tsx` for why D10's no-component-tests line was redrawn.
 */

import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { levelRank, toProjectId, type Level, type Project } from "../../lib/types";
import { ProjectRow } from "./ProjectRow";
import { readActivity, type Activity } from "./projectSignals";

afterEach(cleanup);

const PROJECT: Project = {
  id: toProjectId("/Users/dev/shop"),
  label: "shop",
  path: "/Users/dev/shop",
  health: "ok",
};

/** An **Activity** as it actually arrives: off the `project:activity` event. */
function activityFrom(payload: Record<string, unknown>): Activity {
  const signal = readActivity({ projectId: PROJECT.id, ...payload });
  if (signal === null) throw new Error("the payload was not readable");
  return signal.activity;
}

function renderRow(activity: Activity | undefined, isSelected = false) {
  return render(
    <ProjectRow
      project={PROJECT}
      activity={activity}
      offlineReason={null}
      isSelected={isSelected}
      isBusy={false}
      onSelect={() => {}}
      onRemove={() => {}}
    />,
    { wrapper: ({ children }) => <ul>{children}</ul> },
  );
}

function badge(): HTMLElement | null {
  return document.querySelector<HTMLElement>(".activity-badge");
}

describe("a Project the user is not reading", () => {
  it("shows how many Entries landed and the worst Level among them", () => {
    // The severity shown must be the highest of the batch, not the last one
    // written. `warning` is both the last key of `counts` and the last
    // alphabetically, so a reading that takes either would look plausible and
    // would under-report a live error.
    const counts = { error: 1, info: 8, warning: 3 };
    const activity = activityFrom({ total: 12, counts, maxLevel: "error" });

    renderRow(activity);

    expect(screen.getByText("12")).toBeTruthy();
    expect(badge()?.dataset.level).toBe("error");
    expect(screen.getByText("12 new Entries, highest level error")).toBeTruthy();

    // A string union has no ordering, so "highest" means highest by `levelRank`.
    // Stated here so the expectation above is a claim about severity and not
    // about a literal.
    const worst = (Object.keys(counts) as readonly Level[]).reduce((a, b) =>
      levelRank(b) > levelRank(a) ? b : a,
    );
    expect(worst).toBe("error");
  });

  it("does not let a fragment with no severity pose as one", () => {
    // `unknown` is the absence of a Level, not a low one (LESSONS 5). It must
    // not be announced as a severity a reader could rank, and it ranks below
    // `debug` so it can never inflate the rollup.
    renderRow(activityFrom({ total: 3, maxLevel: "unknown" }));

    expect(screen.getByText("3")).toBeTruthy();
    expect(screen.getByText("3 new Entries, no severity reported")).toBeTruthy();
    expect(screen.queryByText("3 new Entries, highest level unknown")).toBeNull();
    expect(levelRank("unknown")).toBeLessThan(levelRank("debug"));
  });

  it("caps the count rather than printing an unreadable number", () => {
    // The badge is a signal, not a metric — a five-digit count in a sidebar row
    // tells the reader nothing they cannot get by selecting the Project.
    renderRow(activityFrom({ total: 41_302, maxLevel: "critical" }));

    expect(screen.getByText("999+")).toBeTruthy();
    expect(badge()?.dataset.level).toBe("critical");
  });

  it("shows nothing at all when nothing has been written", () => {
    // Not a `0`. A badge on every row is a badge that says nothing.
    renderRow(undefined);
    expect(badge()).toBeNull();
    cleanup();

    renderRow(activityFrom({ total: 0, maxLevel: "unknown" }));
    expect(badge()).toBeNull();
    expect(screen.queryByText("0")).toBeNull();
  });
});

describe("selecting a Project", () => {
  it("clears its badge — the reader is now looking at it", () => {
    const activity = activityFrom({ total: 12, maxLevel: "error" });

    const { rerender } = renderRow(activity);
    expect(badge()).toBeTruthy();

    // `rerender` re-applies the same `<ul>` wrapper, so only the row is passed.
    rerender(
      <ProjectRow
        project={PROJECT}
        activity={activity}
        offlineReason={null}
        isSelected={true}
        isBusy={false}
        onSelect={() => {}}
        onRemove={() => {}}
      />,
    );

    // Even with the Activity still in hand, a selected Project carries no
    // count: Activity is what has *not* been read (D8).
    expect(badge()).toBeNull();
    expect(screen.queryByText("12")).toBeNull();
  });
});
