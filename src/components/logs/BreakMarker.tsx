/**
 * A **Break** in the **Session Record**.
 *
 * The source discontinued here — it was cleared, or it rotated — so the Entries
 * either side are unrelated in time. Everything above a Break is retained
 * (D3, ADR 0001); this marker is the *only* thing that happens, which is why it
 * has to be legible enough that a reader does not mistake the seam for a gap in
 * the log.
 */

import { memo } from "react";

import type { Break } from "../../lib/types";

interface BreakMarkerProps {
  readonly item: Break;
}

/** What actually happened to the file, in the reader's terms. */
const KIND_COPY = {
  cleared: {
    label: "Cleared",
    detail: "The file was emptied. Everything above is kept.",
  },
  rotated: {
    label: "Rotated",
    detail: "A newer file took over. Everything above is kept.",
  },
} as const;

export const BreakMarker = memo(function BreakMarker({
  item,
}: BreakMarkerProps) {
  const copy = KIND_COPY[item.kind];

  return (
    <li
      className="log-break"
      data-item-id={item.id}
      data-kind={item.kind}
    >
      {/*
        Read as one sentence rather than as four fragments. The li keeps its
        implicit `listitem` role — a `separator` inside a list would break the
        list's content model, and the Break *is* part of the record.
      */}
      <span className="log-stream-sr-only">
        {`${copy.label}. Now reading ${item.file}. ${copy.detail}`}
      </span>
      <span className="log-break__rule" aria-hidden="true" />
      <span className="log-break__body" aria-hidden="true">
        <span className="log-break__label">{copy.label}</span>
        <span className="log-break__file">{item.file}</span>
        <span className="log-break__detail">{copy.detail}</span>
      </span>
      <span className="log-break__rule" aria-hidden="true" />
    </li>
  );
});
