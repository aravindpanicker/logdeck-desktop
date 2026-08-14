/**
 * The **Target** control: what this Project is reading (D5).
 *
 * `Latest` follows the newest file in `storage/logs/` and moves across a
 * rotation with a **Break**. Pinning holds the reader on one named file.
 *
 * **A pinned Target still streams.** The watcher polls the pinned file on the
 * same 300 ms tick and delivers Entries into it exactly as before; what pinning
 * stops is *following rotation*, not tailing. Pin today's file and it keeps
 * filling. Nothing here may call a pinned Target frozen, paused or stopped —
 * that would describe behaviour the app does not have, and would make a reader
 * distrust a live view.
 *
 * The listing is read when the menu opens, not when the picker mounts: see
 * `useLogFiles`. A Project whose folder has gone (D9) fails that read, and the
 * menu shows the failure in place of the list rather than an empty list, which
 * would read as "this Project has no logs".
 */

import {
  useCallback,
  useEffect,
  useId,
  useRef,
  useState,
  type KeyboardEvent,
} from "react";

import { useLogFiles } from "../../hooks/useLogFiles";
import type { LogFile, ProjectId, Target } from "../../lib/types";
import { describeLogFile } from "./formatLogFile";
import { isCurrentTarget, pinnedFile } from "./targetChoice";

interface TargetPickerProps {
  readonly projectId: ProjectId;
  readonly target: Target;
  readonly isRetargeting: boolean;
  readonly setTarget: (target: Target) => Promise<void>;
}

const LATEST_VALUE = "latest";

export function TargetPicker({
  projectId,
  target,
  isRetargeting,
  setTarget,
}: TargetPickerProps) {
  const { files, isLoading, error, refresh } = useLogFiles(projectId);
  const [isOpen, setIsOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLUListElement>(null);
  const menuId = useId();

  const pinned = pinnedFile(target);
  /** An age is only meaningful against a fixed instant; taken as the menu opens. */
  const [openedAt, setOpenedAt] = useState(0);

  const close = useCallback((returnFocus: boolean): void => {
    setIsOpen(false);
    if (returnFocus) triggerRef.current?.focus();
  }, []);

  const open = useCallback((): void => {
    setOpenedAt(Math.floor(Date.now() / 1000));
    setIsOpen(true);
    // Fresher than a listing taken at mount, and cheaper than one taken for
    // every Project the reader merely clicks past.
    void refresh();
  }, [refresh]);

  // Dismissal. A pointer landing outside closes the menu; so does Escape from
  // anywhere inside it. Never `window.confirm` — a browser modal blocks the
  // WebView's event loop and the stream stops arriving behind it.
  useEffect(() => {
    if (!isOpen) return;
    const onPointerDown = (event: PointerEvent): void => {
      const root = rootRef.current;
      if (root !== null && !root.contains(event.target as Node)) {
        setIsOpen(false);
      }
    };
    document.addEventListener("pointerdown", onPointerDown);
    return () => document.removeEventListener("pointerdown", onPointerDown);
  }, [isOpen]);

  // Focus moves into the menu on open, onto the item that is currently being
  // read, so a keyboard reader lands where they are rather than at the top.
  useEffect(() => {
    if (!isOpen) return;
    const menu = menuRef.current;
    if (menu === null) return;
    const current = menu.querySelector<HTMLElement>('[aria-checked="true"]');
    (current ?? menu.querySelector<HTMLElement>("[role='menuitemradio']"))
      ?.focus();
  }, [isOpen, files, error]);

  const onMenuKeyDown = useCallback(
    (event: KeyboardEvent<HTMLUListElement>): void => {
      if (event.key === "Escape") {
        event.preventDefault();
        close(true);
        return;
      }
      if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;

      const menu = menuRef.current;
      if (menu === null) return;
      const items = [
        ...menu.querySelectorAll<HTMLElement>("[role='menuitemradio']"),
      ];
      if (items.length === 0) return;
      event.preventDefault();
      const at = items.indexOf(document.activeElement as HTMLElement);
      const step = event.key === "ArrowDown" ? 1 : -1;
      const next = (at + step + items.length) % items.length;
      items[next].focus();
    },
    [close],
  );

  const choose = useCallback(
    (next: Target): void => {
      close(true);
      void setTarget(next);
    },
    [close, setTarget],
  );

  const renderItem = (file: LogFile | null) => {
    const isCurrent = isCurrentTarget(target, file);
    const value = file === null ? LATEST_VALUE : file.name;
    return (
      <li key={value}>
        <button
          type="button"
          role="menuitemradio"
          aria-checked={isCurrent}
          className="target-picker__item"
          data-current={isCurrent ? "true" : undefined}
          onClick={() =>
            choose(file === null ? "latest" : { file: file.name })
          }
        >
          {/* A glyph, not a tint: the current Target is not conveyed by colour
              alone, and `aria-checked` carries it to a screen reader. */}
          <span className="target-picker__mark" aria-hidden="true">
            {isCurrent ? "▸" : ""}
          </span>
          <span className="target-picker__item-name">
            {file === null ? "Latest" : file.name}
          </span>
          <span className="target-picker__item-meta">
            {file === null
              ? "follows the newest file"
              : describeLogFile(file, openedAt)}
          </span>
        </button>
      </li>
    );
  };

  return (
    <div className="target-picker" ref={rootRef}>
      <button
        type="button"
        ref={triggerRef}
        className="target-picker__trigger"
        aria-haspopup="menu"
        aria-expanded={isOpen}
        aria-controls={isOpen ? menuId : undefined}
        data-pinned={pinned !== null ? "true" : undefined}
        disabled={isRetargeting}
        onClick={() => (isOpen ? close(false) : open())}
      >
        <span className="target-picker__label">Target</span>
        <span className="target-picker__value">
          {isRetargeting ? "Switching…" : (pinned ?? "Latest")}
        </span>
        {/* The state is in the word, not only in the styling. */}
        <span className="target-picker__state">
          {pinned !== null ? "pinned" : "following"}
        </span>
        <span className="target-picker__caret" aria-hidden="true">
          ▾
        </span>
      </button>

      {isOpen && (
        <div className="target-picker__menu" id={menuId}>
          <p className="target-picker__note">
            A pinned file keeps streaming. Pinning stops following rotation, not
            tailing.
          </p>

          {error !== null ? (
            <p className="target-picker__error" role="alert">
              {error}
            </p>
          ) : (
            <ul
              className="target-picker__list"
              ref={menuRef}
              role="menu"
              aria-label="Choose the file to read"
              onKeyDown={onMenuKeyDown}
            >
              {renderItem(null)}
              {files.map((file) => renderItem(file))}
            </ul>
          )}

          <p className="target-picker__status" aria-live="polite">
            {isLoading
              ? "Reading the log directory…"
              : error !== null
                ? "The listing could not be read."
                : `${files.length} file${files.length === 1 ? "" : "s"} in storage/logs`}
          </p>
        </div>
      )}
    </div>
  );
}
