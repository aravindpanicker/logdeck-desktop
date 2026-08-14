/**
 * A hand-written stand-in for the Tauri event/command bridge.
 *
 * The hook takes its transport as an argument, so these tests never load
 * `@tauri-apps/api` and never need a Tauri runtime. `emit` delivers
 * synchronously, which is what lets a test assert the buffer's state on the
 * line after the event rather than racing a scheduler.
 */

import type { LogTransport, Unlisten } from "../transport";

export interface InvokeCall {
  readonly command: string;
  readonly args?: Record<string, unknown>;
}

export interface FakeTransport extends LogTransport {
  emit(event: string, payload: unknown): void;
  listenerCount(event: string): number;
  reply(command: string, value: unknown): void;
  fail(command: string, reason: unknown): void;
  /** Makes `listen` for this event reject, leaving the other subscription live. */
  failListen(event: string, reason: unknown): void;
  /** Holds subsequent invokes of this command open until `settle` is called. */
  defer(command: string): void;
  /** Resolves everything `defer` is holding for this command. */
  settle(command: string, value: unknown): void;
  readonly calls: readonly InvokeCall[];
}

export function createFakeTransport(): FakeTransport {
  const listeners = new Map<string, Set<(payload: never) => void>>();
  const listenFailures = new Map<string, unknown>();
  const replies = new Map<string, unknown>();
  const failures = new Map<string, unknown>();
  const deferred = new Set<string>();
  const pending = new Map<string, ((value: unknown) => void)[]>();
  const calls: InvokeCall[] = [];

  return {
    async listen<P>(
      event: string,
      handler: (payload: P) => void,
    ): Promise<Unlisten> {
      if (listenFailures.has(event)) throw listenFailures.get(event);
      const forEvent = listeners.get(event) ?? new Set();
      forEvent.add(handler as (payload: never) => void);
      listeners.set(event, forEvent);
      return () => {
        forEvent.delete(handler as (payload: never) => void);
      };
    },

    async invoke<R>(
      command: string,
      args?: Record<string, unknown>,
    ): Promise<R> {
      calls.push({ command, args });
      if (failures.has(command)) {
        throw failures.get(command);
      }
      if (deferred.has(command)) {
        return new Promise<R>((resolve) => {
          const waiting = pending.get(command) ?? [];
          waiting.push((value) => resolve(value as R));
          pending.set(command, waiting);
        });
      }
      return (replies.get(command) ?? []) as R;
    },

    emit(event: string, payload: unknown): void {
      const forEvent = listeners.get(event);
      if (!forEvent) return;
      for (const handler of [...forEvent]) {
        (handler as (value: unknown) => void)(payload);
      }
    },

    listenerCount(event: string): number {
      return listeners.get(event)?.size ?? 0;
    },

    reply(command: string, value: unknown): void {
      // A later `reply` retracts an earlier `fail`, so a test can model a
      // transient failure followed by recovery.
      failures.delete(command);
      replies.set(command, value);
    },

    fail(command: string, reason: unknown): void {
      failures.set(command, reason);
    },

    failListen(event: string, reason: unknown): void {
      listenFailures.set(event, reason);
    },

    defer(command: string): void {
      deferred.add(command);
    },

    settle(command: string, value: unknown): void {
      deferred.delete(command);
      const waiting = pending.get(command) ?? [];
      pending.delete(command);
      for (const resolve of waiting) resolve(value);
    },

    calls,
  };
}
