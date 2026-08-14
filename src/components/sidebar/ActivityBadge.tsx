/**
 * How much was written to a **Project** the user is not reading, and how bad
 * (D8) — never what.
 *
 * The Level is carried as the same rail token the stream uses, so severity is
 * read the same way in both places. `data-level` resolves `--color-rail` in
 * `global.css`; the badge never names a `--level-*` token itself.
 */

import type { Activity } from "./projectSignals";
import { describeActivity, formatActivityCount } from "./projectSignals";

interface ActivityBadgeProps {
  readonly activity: Activity;
}

export function ActivityBadge({ activity }: ActivityBadgeProps) {
  return (
    <span className="activity-badge" data-level={activity.maxLevel}>
      <span className="activity-badge__rail" aria-hidden="true" />
      <span className="activity-badge__count" aria-hidden="true">
        {formatActivityCount(activity.total)}
      </span>
      <span className="sidebar-sr-only">{describeActivity(activity)}</span>
    </span>
  );
}
