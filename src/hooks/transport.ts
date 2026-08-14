/**
 * The seam between the stream hooks and Tauri.
 *
 * `useLogStream` takes its transport as an argument so the hook can be exercised
 * without a Tauri runtime — a fake pushes events synchronously in tests. The
 * real implementation reaches `@tauri-apps/api` through a dynamic import, so
 * merely loading this module outside the desktop shell does not pull the bridge
 * in.
 */

export type Unlisten = () => void;

export interface LogTransport {
  /** Subscribes to a Tauri event, handing the handler the payload directly. */
  listen<P>(event: string, handler: (payload: P) => void): Promise<Unlisten>;
  /** Invokes a `#[tauri::command]`. Argument keys are camelCase (§3). */
  invoke<R>(command: string, args?: Record<string, unknown>): Promise<R>;
}

/**
 * Turns whatever the bridge threw into something a reader can act on.
 *
 * Rust's `Err(String)` arrives as a bare string and is the message worth showing
 * verbatim — it names the file or the Project that failed. An `Error` is what a
 * fake transport or a bug in this layer throws. Anything else has no message to
 * borrow, so the caller's `fallback` says which read failed.
 *
 * It lives here, at the seam, because both stream hooks stringify the *same*
 * bridge failures: a copy per hook would drift the moment Tauri's error shape
 * changes and only one copy is fixed.
 */
export function describeIpcError(cause: unknown, fallback: string): string {
  if (typeof cause === "string") return cause;
  if (cause instanceof Error) return cause.message;
  return fallback;
}

export const tauriTransport: LogTransport = {
  async listen<P>(
    event: string,
    handler: (payload: P) => void,
  ): Promise<Unlisten> {
    const { listen } = await import("@tauri-apps/api/event");
    return listen<P>(event, (received) => handler(received.payload));
  },

  async invoke<R>(command: string, args?: Record<string, unknown>): Promise<R> {
    const { invoke } = await import("@tauri-apps/api/core");
    return invoke<R>(command, args);
  },
};
