import { LEVELS } from "../../lib/types";
import "./empty-state.css";

/**
 * Shown in the stream region while no **Project** is selected.
 *
 * It doubles as the app's only legend: the Level rail colours are the visual
 * language of every row that follows, so the user has already met them by the
 * time the first Entry arrives.
 */
export function EmptyState() {
  return (
    <div className="empty-state">
      <div className="empty-state__panel">
        <p className="empty-state__eyebrow">No Project selected</p>

        <h2 className="empty-state__title">
          Pick a Project to start its Session Record
        </h2>

        <p className="empty-state__body">
          Register a Laravel project root and LogDeck reads{" "}
          <code className="empty-state__code">storage/logs/</code> inside it,
          following the newest file as it rotates. Entries arrive whole — header,
          context, and every stack frame — so copying one copies all of it.
        </p>

        <dl className="empty-state__legend">
          <dt className="empty-state__legend-term">Level</dt>
          <dd className="empty-state__legend-detail">
            <ul className="empty-state__levels">
              {LEVELS.map((level) => (
                <li key={level} className="empty-state__level" data-level={level}>
                  {level}
                </li>
              ))}
            </ul>
          </dd>
        </dl>
      </div>
    </div>
  );
}
