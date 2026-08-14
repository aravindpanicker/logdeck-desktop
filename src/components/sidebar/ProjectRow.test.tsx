/**
 * An unhealthy **Project** is shown, dimmed, with its reason inline (D4).
 *
 * Registration warns, never blocks — so the failure mode this file exists to
 * catch is a Project the user registered that they cannot find afterwards, or
 * one they can find with nothing on it saying why it is inert. `projectSignals`
 * already pins the *sentences*; what only a rendered row can show is that the
 * sentence reaches the screen and that the row is still in the list.
 *
 * D10 excluded component tests. That exclusion was drawn too tight: this
 * behaviour and the **Activity** badge cannot be verified any other way, and
 * auto-scroll shipped broken behind exactly such a gap (LESSONS 11). This is a
 * narrow layer for those two behaviours, not a policy of asserting markup.
 */

import { cleanup, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { toProjectId, type Health, type Project } from "../../lib/types";
import { ProjectList } from "./ProjectList";
import { ProjectRow } from "./ProjectRow";

afterEach(cleanup);

function project(label: string, health: Health): Project {
  return {
    id: toProjectId(`/Users/dev/${label}`),
    label,
    path: `/Users/dev/${label}`,
    health,
  };
}

function renderRow(
  subject: Project,
  overrides: { readonly offlineReason?: string | null } = {},
) {
  return render(
    <ProjectRow
      project={subject}
      activity={undefined}
      offlineReason={overrides.offlineReason ?? null}
      isSelected={false}
      isBusy={false}
      onSelect={() => {}}
      onRemove={() => {}}
    />,
    { wrapper: ({ children }) => <ul>{children}</ul> },
  );
}

/** The row the stylesheet dims. `data-inert` is the hook it dims through. */
function rowOf(label: string): HTMLElement {
  const row = screen.getByTitle(`/Users/dev/${label}`).closest("li");
  if (row === null) throw new Error(`no row rendered for ${label}`);
  return row;
}

/**
 * D4's "never hidden" half, asserted rather than assumed.
 *
 * `getByText` does not filter on visibility: a `display: none`, a
 * `visibility: hidden`, or an `aria-hidden` wrapper on `.project-row__note`
 * leaves it green, so on its own it pins only "the sentence is somewhere in the
 * tree" — a strictly weaker claim than "the reader can see it", and the weaker
 * one is not the one D4 makes. Two independent checks close the difference:
 *
 * 1. Nothing on the path from the text to the document root hides it — checked
 *    against the *computed* style, so an inline `style` or a stylesheet jsdom
 *    has loaded is caught, and `hidden` / `aria-hidden` are caught outright.
 * 2. The sentence is part of the row button's **accessible name**. That name is
 *    computed from the accessibility tree, which drops hidden subtrees by
 *    either mechanism, so it fails independently of check 1's property list.
 *
 * What it still cannot see is a class whose rule lives in `sidebar.css`, which
 * is never loaded here — dimming and colour stay human (Manual verification 3).
 */
function expectReadable(sentence: string, scope?: HTMLElement): void {
  const area = scope === undefined ? screen : within(scope);
  const node = area.getByText(sentence);

  for (let el: HTMLElement | null = node; el !== null; el = el.parentElement) {
    const at = `${el.tagName.toLowerCase()}.${el.className || "(no class)"}`;
    const style = getComputedStyle(el);
    expect(style.display, `${at} hides the reason with display`).not.toBe("none");
    expect(style.visibility, `${at} hides the reason with visibility`).not.toBe(
      "hidden",
    );
    expect(style.visibility, `${at} hides the reason with visibility`).not.toBe(
      "collapse",
    );
    expect(style.opacity, `${at} hides the reason with opacity`).not.toBe("0");
    expect(el.hidden, `${at} carries the hidden attribute`).toBe(false);
    expect(
      el.getAttribute("aria-hidden"),
      `${at} hides the reason from the accessibility tree`,
    ).not.toBe("true");
  }

  const named = area.queryAllByRole("button", {
    name: (accessibleName: string) => accessibleName.includes(sentence),
  });
  expect(
    named.length,
    `no row announces "${sentence}" in its accessible name`,
  ).toBeGreaterThan(0);
}

describe("an unhealthy Project's row", () => {
  it("tells the reader why the folder they registered is inert", () => {
    // Each Health variant, as the sentence a user reads — not a variant name.
    // A row that renders "noLogsDir" would technically be showing a reason and
    // would still leave the reader with nothing to act on.
    const cases: readonly (readonly [Health, string])[] = [
      ["noLogsDir", "no storage/logs found"],
      ["notLaravel", "not a Laravel project"],
      [{ unavailable: "/Volumes/work is not mounted" }, "/Volumes/work is not mounted"],
    ];

    for (const [health, sentence] of cases) {
      renderRow(project("shop", health));
      expectReadable(sentence);
      // Dimmed, not hidden: the row is on screen and carries the attribute
      // `sidebar.css` dims it through. The opacity itself is a human check.
      expect(rowOf("shop").dataset.inert).toBe("true");
      cleanup();
    }
  });

  it("gives an unavailable Project its own reason, not a stock one", () => {
    // Unavailable(String) is the one Health variant carrying a payload, and the
    // payload is the whole point of it: two Projects unavailable for different
    // reasons must not read identically.
    renderRow(project("shop", { unavailable: "permission denied" }));
    expectReadable("permission denied");
    expect(screen.queryByText("source unavailable")).toBeNull();
    cleanup();

    renderRow(project("shop", { unavailable: "/Volumes/work is not mounted" }));
    expectReadable("/Volumes/work is not mounted");
    expect(screen.queryByText("permission denied")).toBeNull();
  });

  it("says the source is unreachable now rather than what was true at registration", () => {
    // D9's live counterpart. A Project registered healthy whose folder has been
    // moved away must not read as fine.
    renderRow(project("shop", "ok"), { offlineReason: "folder moved away" });

    expectReadable("folder moved away");
    expect(rowOf("shop").dataset.inert).toBe("true");
  });

  it("says nothing under a healthy Project", () => {
    // The counterweight: a note on every row would make the note meaningless.
    renderRow(project("shop", "ok"));

    const row = rowOf("shop");
    expect(row.dataset.inert).toBeUndefined();
    expect(row.querySelector(".project-row__note")).toBeNull();
  });
});

describe("the sidebar list", () => {
  it("still lists a Project whose folder is not a Laravel app", () => {
    // The D4 symptom, and the one that cannot be seen from a single row:
    // registration warns, never blocks, so an unhealthy Project is in the list
    // beside the healthy ones rather than filtered out of it.
    const projects = [
      project("shop", "ok"),
      project("scratch", "notLaravel"),
      project("archive", { unavailable: "/Volumes/work is not mounted" }),
    ];

    render(
      <ProjectList
        projects={projects}
        activity={new Map()}
        offline={new Map()}
        selectedProjectId={null}
        isLoading={false}
        isBusy={false}
        error={null}
        onSelect={() => {}}
        onRemove={() => {}}
        onDismissError={() => {}}
      />,
    );

    const list = screen.getByRole("list");
    expect(within(list).getAllByRole("listitem")).toHaveLength(3);
    for (const label of ["shop", "scratch", "archive"]) {
      expect(within(list).getByText(label)).toBeTruthy();
    }
    expectReadable("not a Laravel project", list);
    expectReadable("/Volumes/work is not mounted", list);
  });
});
