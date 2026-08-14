/**
 * One **Project** in the sidebar.
 *
 * An unhealthy Project is shown, dimmed, with its reason inline — never hidden
 * and never rejected (D4). An offline one dims the same way and undims by
 * itself when the path returns (D9), so the row has no "retry" control: there
 * is nothing for the user to do.
 *
 * Removal is deliberate but never modal. `window.confirm` blocks the WebView
 * and takes the event loop with it, so the confirmation is an inline affordance
 * that the row owns and Escape dismisses.
 */

import { memo, useEffect, useRef, useState } from "react";

import type { Project, ProjectId } from "../../lib/types";
import { ActivityBadge } from "./ActivityBadge";
import {
  describeHealth,
  shouldShowActivityBadge,
  type Activity,
} from "./projectSignals";

interface ProjectRowProps {
  readonly project: Project;
  /** Absent when nothing has been written since the app opened. */
  readonly activity: Activity | undefined;
  /** Set while the source cannot be read; clears on its own (D9). */
  readonly offlineReason: string | null;
  readonly isSelected: boolean;
  readonly isBusy: boolean;
  readonly onSelect: (projectId: ProjectId) => void;
  readonly onRemove: (projectId: ProjectId) => void;
}

function ProjectRowInner({
  project,
  activity,
  offlineReason,
  isSelected,
  isBusy,
  onSelect,
  onRemove,
}: ProjectRowProps) {
  const [isConfirming, setIsConfirming] = useState(false);
  const confirmRef = useRef<HTMLButtonElement>(null);
  const removeRef = useRef<HTMLButtonElement>(null);

  // Opening the confirmation moves focus into it, so the keyboard path and the
  // pointer path end in the same place.
  useEffect(() => {
    if (isConfirming) {
      confirmRef.current?.focus();
    }
  }, [isConfirming]);

  // An offline source is the more urgent fact, so it wins over the stored
  // Health — which was computed when the Project was registered or restored.
  const note = offlineReason ?? describeHealth(project.health);
  const isInert = note !== null;
  const badge = shouldShowActivityBadge(isSelected, activity);

  const cancel = (): void => {
    setIsConfirming(false);
    removeRef.current?.focus();
  };

  return (
    <li
      className="project-row"
      data-selected={isSelected ? "true" : undefined}
      data-inert={isInert ? "true" : undefined}
      data-confirming={isConfirming ? "true" : undefined}
    >
      <div className="project-row__main">
        <button
          type="button"
          className="project-row__select"
          data-project-row=""
          aria-current={isSelected ? "true" : undefined}
          onClick={() => onSelect(project.id)}
          title={project.path}
        >
          <span className="project-row__rail" aria-hidden="true" />
          <span className="project-row__caret" aria-hidden="true">
            {isSelected ? "▸" : ""}
          </span>
          <span className="project-row__text">
            <span className="project-row__label">{project.label}</span>
            {note !== null && (
              <span className="project-row__note" title={note}>
                {note}
              </span>
            )}
          </span>
          {badge && <ActivityBadge activity={activity} />}
        </button>

        <button
          type="button"
          ref={removeRef}
          className="project-row__remove"
          onClick={() => setIsConfirming(true)}
          disabled={isBusy || isConfirming}
          aria-expanded={isConfirming}
        >
          <span aria-hidden="true">{"×"}</span>
          <span className="sidebar-sr-only">Remove {project.label}</span>
        </button>
      </div>

      {isConfirming && (
        <div
          className="project-row__confirm"
          role="group"
          aria-label={`Remove ${project.label}`}
          onKeyDown={(event) => {
            if (event.key === "Escape") {
              event.stopPropagation();
              cancel();
            }
          }}
        >
          <p className="project-row__confirm-copy">
            Stop watching this Project? Its log files are untouched.
          </p>
          <div className="project-row__confirm-actions">
            <button
              type="button"
              ref={confirmRef}
              className="project-row__confirm-yes"
              onClick={() => {
                setIsConfirming(false);
                onRemove(project.id);
              }}
              disabled={isBusy}
            >
              Remove
            </button>
            <button
              type="button"
              className="project-row__confirm-no"
              onClick={cancel}
            >
              Cancel
            </button>
          </div>
        </div>
      )}
    </li>
  );
}

/**
 * Memoised on purpose. Every registered **Project** is watched (D8), so a
 * `project:activity` or `project:status` event arrives for *any* of them every
 * few hundred milliseconds, and each one replaces the whole `activity` / offline
 * map. Without a memo boundary one Project's tick re-renders every other
 * Project's row, for the lifetime of the session, for nothing. The row's props
 * are the values pulled out of those maps plus two callbacks the shell holds
 * stable, so a shallow comparison is exactly right here.
 */
export const ProjectRow = memo(ProjectRowInner);
