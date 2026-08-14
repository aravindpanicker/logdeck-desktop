/**
 * Putting a whole **Entry** on the clipboard (D1).
 *
 * One copy action is one logical event: `entry.raw` is the verbatim text the
 * watcher captured — header, JSON context, and every stack frame — which is why
 * `raw` is stored rather than reassembled from the parsed parts (§2). Copying
 * the visible line only would hand the reader something they cannot paste into a
 * bug report.
 *
 * The confirmation is a transient state on the row plus a polite announcement.
 * It is deliberately not a dialog: `window.alert` blocks the WebView and takes
 * the live stream down with it.
 */

import { useCallback, useEffect, useRef, useState } from "react";

import type { EntryId, LogEntry } from "../../lib/types";

/** How long the row says "Copied" before returning to rest. */
const CONFIRM_MS = 1600;

export interface CopyEntry {
  /** The Entry currently confirming, or `null`. */
  readonly copiedId: EntryId | null;
  /** Announced politely; carries the failure reason when a copy fails. */
  readonly notice: string | null;
  copy(entry: LogEntry): void;
}

function describeError(cause: unknown): string {
  if (typeof cause === "string") return cause;
  if (cause instanceof Error) return cause.message;
  return "an unknown failure";
}

export function useCopyEntry(): CopyEntry {
  const [copiedId, setCopiedId] = useState<EntryId | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const aliveRef = useRef(true);

  useEffect(() => {
    aliveRef.current = true;
    return () => {
      aliveRef.current = false;
      if (timerRef.current !== null) clearTimeout(timerRef.current);
    };
  }, []);

  const copy = useCallback((entry: LogEntry): void => {
    void (async () => {
      try {
        // Imported on demand, like the stream transport: loading this module
        // outside the desktop shell must not pull the Tauri bridge in.
        const { writeText } = await import(
          "@tauri-apps/plugin-clipboard-manager"
        );
        await writeText(entry.raw);
        if (!aliveRef.current) return;

        setCopiedId(entry.id);
        setNotice("Entry copied — header, context, and every frame.");
        if (timerRef.current !== null) clearTimeout(timerRef.current);
        timerRef.current = setTimeout(() => {
          if (!aliveRef.current) return;
          setCopiedId(null);
        }, CONFIRM_MS);
      } catch (cause) {
        if (!aliveRef.current) return;
        setCopiedId(null);
        setNotice(`Copy failed: ${describeError(cause)}`);
      }
    })();
  }, []);

  return { copiedId, notice, copy };
}
