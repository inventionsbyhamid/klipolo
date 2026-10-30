/** The project and revision an edit's ops were built on: Rust refuses a stale one with `project.moved`. */
export interface ApplyBase {
  projectId: string | null;
  baseRevision: number | null;
}

const NONE: ApplyBase = { projectId: null, baseRevision: null };
let current = NONE;

/** What `bridge.apply` sends with the edit it is sending now; nothing (Rust checks nothing) outside `withBase`. */
export function applyBase(): ApplyBase {
  return current;
}

/**
 * Sends an edit with `base`: `send` calls `bridge.apply`, which reads it at once. The project store decides the base
 * (`state/project`). This lives in its own module because tests mock `bridge/commands`, and `bridge.apply` keeps its
 * one argument.
 */
export function withBase<T>(base: ApplyBase, send: () => T): T {
  current = base;
  try {
    return send();
  } finally {
    current = NONE;
  }
}
