import { useCallback, useEffect, useRef } from 'react';
import type { Op } from '../../bindings/Op';
import type { Label } from '../../lib/labels';
import { registerBurst } from '../../state/bursts';
import type { Built } from '../../state/editBase';
import { captureBase } from '../../state/project';
import { clearPreview, dropPendingPreview, previewOps } from './livePreview';
import { queueCommit } from '../../state/commit';

/** A burst of quick changes commits as one undo step this long after the last one. */
export const SETTLE_MS = 400;

export interface BurstChange {
  /** Its undo label: a message id, with the values it shows. */
  label: Label;
  /**
   * The ops, or a builder for them: called when the burst commits (and once a frame for its dry run), so ops that
   * carry a whole value (a layer's content, a point) build on the project as it is then, a change made by another
   * control in the meantime included. (A change from elsewhere, such as chat's, merged since the press makes Rust
   * refuse it: L7.)
   */
  ops: Op[] | (() => Op[]);
  /** Nothing changes after all (nudged back to where it began): drop the dry run instead of committing. */
  noop?: boolean;
  /** Dry-run it in the engine (off for changes the picture doesn't show, like volume). */
  preview?: boolean;
  /** After the commit (true when it went in, or there was nothing to commit). */
  done?: (ok: boolean) => void;
}

/** A pending change, with where it was decided: at its press. */
type Pending = BurstChange & { built: Built };

/**
 * Coalesces quick changes (arrow nudges, key steps on a slider, − and + presses, a colour being picked) into one
 * undo step. Each change is dry-run in the engine straight away (at most once per frame); the newest is
 * committed `SETTLE_MS` after the last, or at once on `flush` (blur, a closing popover, another key), on unmount and
 * before an undo or redo (`flushBursts`). It goes in through the commit queue (`queueCommit`): built once a commit on
 * its way has landed, and holding the queue until it lands, so a control used straight after builds on it. `commit`
 * sends the control's other changes (a slider drag's release) through the same queue, in order with its bursts. Every
 * one is kept at close: as the project closes or the app quits, a commit still waiting goes
 * in its turn, the close waiting for it, never at once ahead of the one it waits for, so a change that carries a whole
 * value (the caption settings, a layer's content) never puts back what that one changed.
 */
export function useBurst() {
  const pending = useRef<Pending | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout>>(undefined);

  /** One undo step through the commit queue, in its turn, kept at close (the close waits for it: `keepAtClose`). */
  const enqueue = useCallback(
    (label: Label, build: () => Op[], built?: Built) =>
      queueCommit(label, build, built, { keep: label }).landed,
    [],
  );

  /**
   * Commits the pending change; true when there was one. Closing or not, it goes in its turn: closing keeps it (and
   * this control's earlier commits still waiting), so the newest change never lands under an older one, nor ahead of
   * a commit it waits for.
   */
  const send = useCallback(() => {
    clearTimeout(timer.current);
    const change = pending.current;
    pending.current = null;
    if (!change) return false;
    const previewed = change.preview !== false;
    if (change.noop) {
      if (previewed) clearPreview();
      change.done?.(true);
      return true;
    }
    // The new revision expires the dry run, so the frame never flickers back.
    if (previewed) dropPendingPreview();
    const { ops } = change;
    void enqueue(change.label, typeof ops === 'function' ? ops : () => ops, change.built).then((ok) => {
      if (!ok && previewed) clearPreview();
      change.done?.(ok);
    });
    return true;
  }, [enqueue]);
  const flush = useCallback(() => void send(), [send]);
  const commit = useCallback((label: Label, build: () => Op[]) => enqueue(label, build), [enqueue]);
  // Unmounted mid-burst (the selection or the playhead moved on): keep what was done.
  useEffect(() => flush, [flush]);
  // Undo, redo, export and closing commit a pending burst first.
  useEffect(() => registerBurst(send), [send]);

  const push = useCallback(
    (change: BurstChange) => {
      // Decided at this press, on the copy as it is now (ready-made ops, or a builder closing over this render's
      // values): a chat change merged before it goes in makes Rust refuse it. This window's own changes landing
      // meanwhile don't: a builder still builds on them.
      pending.current = { ...change, built: captureBase() };
      if (change.preview !== false) previewOps(change.ops);
      clearTimeout(timer.current);
      timer.current = setTimeout(flush, SETTLE_MS);
    },
    [flush],
  );
  return { push, flush, commit, pending: () => pending.current !== null };
}
