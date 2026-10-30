import type { Snapshot } from '../bindings/Snapshot';
import type { ApplyBase } from '../bridge/applyBase';

/**
 * Where an edit's ops were built (refuse, never rebase): the project and revision of this window's copy then,
 * and how many changes from elsewhere it had seen. Every edit source takes it when it builds its ops: `useProject.apply`
 * at its call, a held key or toolbar burst at its press (`captureBase`), undo and redo at their turn.
 */
export interface Built {
  projectId: string | null;
  revision: number | null;
  foreign: number;
}

/**
 * Changes this window didn't make that reached its copy: a chat or MCP edit merged, the project fetched again (a
 * `project.moved` resync, a patch that didn't follow; counted when asked for and again when it arrives), another
 * project opened.
 */
let foreign = 0;

export function changedElsewhere() {
  foreign += 1;
}

/** Whether a change from elsewhere reached the copy since `built` was taken. */
export function changedSince(built: Built): boolean {
  return built.foreign !== foreign;
}

export function capture(snapshot: Snapshot | null): Built {
  return { projectId: snapshot?.project.id ?? null, revision: snapshot?.history.revision ?? null, foreign };
}

/**
 * What an edit built at `built` is sent with, the copy now being `snapshot`. When only this window's own edits landed
 * since (queued commits), its ops follow them: the current revision. After a change from elsewhere, the revision
 * its ops were built on, which Rust refuses with `project.moved`.
 */
export function baseFor(built: Built, snapshot: Snapshot | null): ApplyBase {
  const now = snapshot?.history.revision ?? null;
  return { projectId: built.projectId, baseRevision: built.foreign === foreign ? now : built.revision };
}
