import type { Op } from '../bindings/Op';
import { engine } from '../bridge/engine';
import { deepEqual } from '../lib/equal';
import { useProject } from './project';

/** Ops, or a builder for them: a drag passes a builder so the ops are made once per frame, not per pointer event. */
export type PreviewOps = Op[] | (() => Op[]);

let pending: PreviewOps | null = null;
let scheduled = false;
/** The dry run the engine has: the same ops on the same revision again change nothing, so they aren't sent. */
let sent: { projectId: string; revision: number; ops: Op[] } | null = null;
const nextFrame = (cb: () => void) =>
  typeof requestAnimationFrame === 'function' ? requestAnimationFrame(cb) : setTimeout(cb, 16);

/**
 * Dry-run `ops` in the engine at most once per animation frame; the newest ops win. Ops deep-equal to the dry
 * run the engine already has for this revision are skipped: a slider or pointer that moved without changing the value
 * costs a comparison of a few small ops, not an IPC call and a re-render of the frame.
 */
export function previewOps(ops: PreviewOps) {
  pending = ops;
  if (scheduled) return;
  scheduled = true;
  nextFrame(() => {
    scheduled = false;
    const next = pending;
    pending = null;
    const snapshot = useProject.getState().snapshot;
    if (!snapshot || !next) return;
    const list = typeof next === 'function' ? next() : next;
    const projectId = snapshot.project.id;
    const revision = snapshot.history.revision;
    if (sent && sent.projectId === projectId && sent.revision === revision && deepEqual(sent.ops, list))
      return;
    const now = { projectId, revision, ops: list };
    sent = now;
    void engine.preview(projectId, revision, list).catch(() => {
      // Not drawn after all: the same ops again must go out.
      if (sent === now) sent = null;
    });
  });
}

/**
 * Forget a dry run that hasn't gone out yet, because the same ops are being committed: the new revision expires
 * whatever the engine already has (no clear, so the frame never flickers back).
 */
export function dropPendingPreview() {
  pending = null;
}

/** Drop the dry run (Escape, a failed commit, or a drag that ended where it began). */
export function clearPreview() {
  pending = null;
  sent = null;
  void engine.previewClear().catch(() => undefined);
}
