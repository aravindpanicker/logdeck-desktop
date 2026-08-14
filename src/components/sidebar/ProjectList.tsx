/**
 * The registered **Projects**, in registry order.
 *
 * A real `<ul>`: the count is announced, and the rows are one list rather than
 * a stack of buttons. Every Project is listed whatever its **Health** — the
 * list is the user's record of what they registered, not a filtered view of
 * what happens to work right now (D4).
 */

import { useRef, type KeyboardEvent } from "react";

import type { Project, ProjectId } from "../../lib/types";
import { ProjectRow } from "./ProjectRow";
import type { Activity } from "./projectSignals";
import "./sidebar.css";

interface ProjectListProps {
  readonly projects: readonly Project[];
  readonly activity: ReadonlyMap<ProjectId, Activity>;
  readonly offline: ReadonlyMap<ProjectId, string>;
  readonly selectedProjectId: ProjectId | null;
  readonly isLoading: boolean;
  readonly isBusy: boolean;
  readonly error: string | null;
  readonly onSelect: (projectId: ProjectId) => void;
  readonly onRemove: (projectId: ProjectId) => void;
  readonly onDismissError: () => void;
}

/** Up and down move between rows; Home and End jump to the ends. */
const ROW_KEYS = new Set(["ArrowDown", "ArrowUp", "Home", "End"]);

function nextIndex(key: string, current: number, count: number): number {
  switch (key) {
    case "ArrowDown":
      return (current + 1) % count;
    case "ArrowUp":
      return (current - 1 + count) % count;
    case "Home":
      return 0;
    default:
      return count - 1;
  }
}

export function ProjectList({
  projects,
  activity,
  offline,
  selectedProjectId,
  isLoading,
  isBusy,
  error,
  onSelect,
  onRemove,
  onDismissError,
}: ProjectListProps) {
  const listRef = useRef<HTMLUListElement>(null);

  // Tab reaches the list; the arrows walk it, which is how a list of sources is
  // expected to behave once you are inside it.
  const handleKeyDown = (event: KeyboardEvent<HTMLUListElement>): void => {
    if (!ROW_KEYS.has(event.key) || listRef.current === null) {
      return;
    }
    const rows = Array.from(
      listRef.current.querySelectorAll<HTMLButtonElement>("[data-project-row]"),
    );
    const current = rows.indexOf(document.activeElement as HTMLButtonElement);
    if (current === -1 || rows.length === 0) {
      return;
    }
    event.preventDefault();
    rows[nextIndex(event.key, current, rows.length)]?.focus();
  };

  // `data-level="error"` on the banner resolves the rail colour through the
  // same mechanism a Level uses, so the failure is coloured without a literal.
  return (
    <div className="project-list">
      {error !== null && (
        <div className="project-list__error" role="alert" data-level="error">
          <p className="project-list__error-text">{error}</p>
          <button
            type="button"
            className="project-list__error-dismiss"
            onClick={onDismissError}
          >
            Dismiss
          </button>
        </div>
      )}

      {isLoading ? (
        <p className="project-list__hint">Reading the registry…</p>
      ) : projects.length === 0 ? (
        <p className="project-list__hint">
          No Projects registered yet. Add a Laravel project root and LogDeck
          watches <code className="project-list__code">storage/logs/</code>{" "}
          inside it.
        </p>
      ) : (
        <ul
          className="project-list__items"
          ref={listRef}
          onKeyDown={handleKeyDown}
        >
          {projects.map((project) => (
            <ProjectRow
              key={project.id}
              project={project}
              activity={activity.get(project.id)}
              offlineReason={offline.get(project.id) ?? null}
              isSelected={project.id === selectedProjectId}
              isBusy={isBusy}
              onSelect={onSelect}
              onRemove={onRemove}
            />
          ))}
        </ul>
      )}
    </div>
  );
}
