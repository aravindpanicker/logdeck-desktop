import { useCallback, useState } from "react";

import { EmptyState } from "./components/logs/EmptyState";
import { LogStream } from "./components/logs/LogStream";
import { AddProjectButton } from "./components/sidebar/AddProjectButton";
import { ProjectList } from "./components/sidebar/ProjectList";
import { useProjects } from "./components/sidebar/useProjects";
import { useProjectSignals } from "./components/sidebar/useProjectSignals";
import type { ProjectId } from "./lib/types";
import "./styles/app.css";

/**
 * The shell: a raised sidebar listing **Projects** and the stream region that
 * renders the selected Project's **Session Record**.
 *
 * The selected `ProjectId` is lifted here because two regions need it — the
 * sidebar to mark the row and suppress its **Activity** badge (D8), and the
 * stream to know what to read.
 */
function App() {
  const {
    projects,
    isLoading,
    isBusy,
    error,
    addProject,
    removeProject,
    reportError,
    dismissError,
  } = useProjects();

  const [selectedProjectId, setSelectedProjectId] = useState<ProjectId | null>(
    null,
  );
  const { activity, offline, signalError } = useProjectSignals(selectedProjectId);

  const handleSelect = useCallback((projectId: ProjectId): void => {
    setSelectedProjectId((current) => (current === projectId ? null : projectId));
  }, []);

  // Removing the Project being read leaves nothing to read; the stream region
  // falls back to its empty state rather than pointing at a gone Project.
  const handleRemove = useCallback(
    (projectId: ProjectId): void => {
      setSelectedProjectId((current) =>
        current === projectId ? null : current,
      );
      void removeProject(projectId);
    },
    [removeProject],
  );

  const watching = projects.length === 1 ? "1 source" : `${projects.length} sources`;

  return (
    <div className="app-shell">
      <nav className="app-sidebar" aria-label="Projects">
        <div className="app-brand">
          <h1 className="app-brand__mark">
            Log<span>Deck</span>
          </h1>
          <p className="app-brand__version">v1</p>
        </div>

        <div className="app-sidebar__section">
          <h2 className="app-sidebar__section-title">Projects</h2>
          <p className="app-sidebar__count">{projects.length}</p>
        </div>

        <div className="app-sidebar__body">
          <ProjectList
            projects={projects}
            activity={activity}
            offline={offline}
            selectedProjectId={selectedProjectId}
            isLoading={isLoading}
            isBusy={isBusy}
            error={error}
            onSelect={handleSelect}
            onRemove={handleRemove}
            onDismissError={dismissError}
          />
        </div>

        <div className="app-sidebar__footer sidebar-footer">
          <AddProjectButton
            onChoose={addProject}
            onError={reportError}
            disabled={isBusy}
          />
          <p className="sidebar-footer__status">Watching {watching}</p>
          {signalError !== null && (
            <p className="sidebar-footer__note">Live updates: {signalError}</p>
          )}
        </div>
      </nav>

      {/*
        Selecting a different Project is a different **Session Record**, not a
        reconfigured one: the `key` remounts the stream so no scroll position,
        filter, or expanded trace leaks from the Project just left.
      */}
      <main className="app-stream" aria-label="Log stream">
        {selectedProjectId === null ? (
          <EmptyState />
        ) : (
          <LogStream key={selectedProjectId} projectId={selectedProjectId} />
        )}
      </main>
    </div>
  );
}

export default App;
