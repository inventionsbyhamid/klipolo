import type { Op } from '../bindings/Op';
import { labelParts, type Label } from '../lib/labels';
import { queueCommit } from './commit';
import { registerBurst, SETTLE_MS } from './bursts';
import { afterCommits, beginCommit, onSettling, type KeptHold } from './commitQueue';
import type { Built } from './editBase';
import { clearPreview, dropPendingPreview, previewOps } from './livePreview';
import { captureBase, useProject } from './project';

/** A keyboard edit settling into one undo step. */
export interface KeyChange {
  /** Its undo label: a message id, with the values it shows. */
  label: Label;
  ops: Op[];
  /** Which edit it is (`nudge:title`): another press of the same key replaces it, any other commits it first. */
  key: string;
  /** What the presses add up to so far (a held nudge's frames), for the next press to build on. */
  total?: number;
  /** Once it has gone in (true) or failed: say what it did, put back what the burst drew. */
  done?: (ok: boolean) => void;
  /** Where its ops were built: at the press, up to `SETTLE_MS` before they go in. */
  built?: Built;
}

let pending: KeyChange | null = null;
let timer: ReturnType<typeof setTimeout> | undefined;

/**
 * Commits the pending change now; true when it had ops to commit. `closing` (the project closing, or the app quitting):
 * in its turn instead, kept at close, so it goes in after a commit on its way and the close waits for it.
 */
function flush(closing = false): boolean {
  clearTimeout(timer);
  const change = pending;
  pending = null;
  if (!change) return false;
  if (change.ops.length === 0) {
    // Nudged back to where it began: nothing to commit, and nothing left to dry-run.
    clearPreview();
    change.done?.(true);
    return false;
  }
  // The new revision expires the dry run, so the frame never flickers back.
  dropPendingPreview();
  const landed = (ok: boolean) => {
    if (!ok) clearPreview();
    change.done?.(ok);
  };
  if (closing) {
    const ops = change.ops;
    void queueCommit(change.label, () => ops, change.built, { keep: change.label }).landed.then(landed);
    return true;
  }
  // It holds the timeline's commit queue until it lands: a press, a drop or another edit waits for it.
  const end = beginCommit();
  const [label, params] = labelParts(change.label);
  void useProject.getState().apply(label, change.ops, params, change.built).then(landed).finally(end);
  return true;
}

// Undo and redo commit a pending change first, so ⌘Z takes back exactly the presses just made; and when they waited
// their turn behind a press that started one meanwhile, it goes in before them too (`yieldToSettling`).
registerBurst(flush);
onSettling(() => flush());

/**
 * `useBurst` for commands (they live outside any component): a held key's presses are one undo step. Each press
 * replaces the pending change of its `key` and is dry-run in the engine at once (at most once a frame); the change
 * commits `SETTLE_MS` after the last press, or at once on `flush` (another edit, a press on a block, undo, unmount).
 */
export const keyBurst = {
  push(label: Label, ops: Op[], key: string, extra: Pick<KeyChange, 'total' | 'done'> = {}) {
    if (pending && pending.key !== key) flush();
    // Its ops are of the copy as it is at this press: a chat change merged before they go in makes Rust refuse them.
    pending = { label, ops, key, ...extra, built: captureBase() };
    if (ops.length > 0) previewOps(ops);
    else clearPreview();
    clearTimeout(timer);
    timer = setTimeout(flush, SETTLE_MS);
  },
  /** The change settling for `key`, if the burst in progress is that key's. */
  pending(key: string): KeyChange | null {
    return pending?.key === key ? pending : null;
  },
  flush,
  /**
   * Runs `action` once what is settling here, and any commit in flight (a drop's, another edit's), has gone in: now
   * when nothing is. An edit built from positions (a trim, a duplicate, a nudge) then builds on where they put things.
   * A burst another waiting press started meanwhile (not `key`'s own, which a nudge adds to) goes in first too, and
   * `action` then runs as soon as it lands, before anything that waited after it. `kept`: the hold an
   * edit took when it was asked for, before it awaited something (`keepAtClose`): a close waits for it
   * in its turn and says it by its label, rather than counting it.
   */
  after(action: () => void, key?: string, kept: KeptHold | null = null) {
    flush();
    const run = () => {
      if (pending && pending.key !== key && flush()) afterCommits(run, { first: true, kept });
      else action();
    };
    afterCommits(run, { kept });
  },
};
