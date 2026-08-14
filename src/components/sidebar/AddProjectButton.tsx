/**
 * Registers a **Project** through the OS folder picker.
 *
 * The native dialog is the only file chooser in the app: **Project** paths are
 * picked at runtime, so the `fs` plugin's compile-time scopes do not fit and
 * every later read goes through our own Rust commands (BUILD-SPEC §7).
 *
 * The picker returns a path; validation is Rust's job and warns rather than
 * blocks (D4), so a folder that turns out not to be Laravel still lands in the
 * list with its reason attached.
 */

import { useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";

interface AddProjectButtonProps {
  /** Never throws: the registry hook owns command failures. */
  readonly onChoose: (path: string) => Promise<void>;
  /** For the dialog's own failures, which no other component can see. */
  readonly onError: (message: string) => void;
  readonly disabled?: boolean;
}

export function AddProjectButton({
  onChoose,
  onError,
  disabled = false,
}: AddProjectButtonProps) {
  const [isPicking, setIsPicking] = useState(false);
  const isDisabled = disabled || isPicking;

  const handleClick = async (): Promise<void> => {
    setIsPicking(true);
    try {
      const chosen = await open({
        directory: true,
        multiple: false,
        title: "Choose a Laravel project folder",
      });
      // Cancelling is not a failure, and must not leave an error on screen.
      if (typeof chosen === "string") {
        await onChoose(chosen);
      }
    } catch (cause: unknown) {
      onError(
        cause instanceof Error
          ? `could not open the folder picker: ${cause.message}`
          : "could not open the folder picker",
      );
    } finally {
      setIsPicking(false);
    }
  };

  return (
    <button
      type="button"
      className="add-project"
      onClick={() => void handleClick()}
      disabled={isDisabled}
      aria-busy={isPicking}
    >
      <span className="add-project__glyph" aria-hidden="true">
        +
      </span>
      <span className="add-project__text">
        {isPicking ? "Choosing…" : "Add Project"}
      </span>
    </button>
  );
}
