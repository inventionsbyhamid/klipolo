import { useEffect, useRef, useState } from 'react';
import type { Op } from '../../bindings/Op';
import { Slider } from '../../design/Slider';
import { clearPreview, dropPendingPreview, previewOps } from '../../state/livePreview';
import { useBurst } from '../preview';
import type { Label } from '../../lib/labels';

interface ToolSliderProps {
  label: string;
  /** The undo label of the one change a drag commits. */
  commitLabel: Label;
  value: number;
  min: number;
  max: number;
  step: number;
  format: (value: number) => string;
  /**
   * The ops that set `value`: called as the change is built (at its turn in the commit queue, and once a frame for its
   * dry run), so they read what they carry whole from the project then (`layoutOps`, `contentOps`).
   */
  ops: (value: number) => Op[];
  /** Dry-run moves in the engine; off for changes the picture doesn't show (volume). */
  preview?: boolean;
  /** Inert (why is said beside it). */
  disabled?: boolean;
}

/** A dragged value, shown while the project still has the value the drag started from. */
interface Draft {
  base: number;
  value: number;
}

/**
 * A slider that edits the project like a gizmo: moves are dry-run in the engine at most once per frame and a
 * drag commits one op on release, so it is one undo step; so is a run of key presses (`useBurst`: committed 400 ms
 * after the last, or on blur). Only this slider re-renders per move. Radix commits only a value that differs from
 * where the drag began; a drag that returns there commits nothing.
 */
export function ToolSlider({ label, commitLabel, value, ops, preview = true, ...range }: ToolSliderProps) {
  const [draft, setDraft] = useState<Draft | null>(null);
  const latest = useRef<Draft | null>(null);
  /** Whether a pointer drag's dry run may be drawn. */
  const previewing = useRef(false);
  /** The project's value when a run of key presses began. */
  const keyBase = useRef(value);
  const burst = useBurst();
  const shown = draft && draft.base === value ? draft.value : value;

  useEffect(() => {
    const drawn = previewing;
    // Unmounted mid-drag (the selection changed): don't leave the dry run drawn in every frame.
    return () => {
      if (drawn.current) clearPreview();
    };
  }, []);

  const settle = (committed: Draft | null) => setDraft((d) => (d === committed ? null : d));

  return (
    <Slider
      label={label}
      value={shown}
      {...range}
      onChange={(v) => {
        const next = { base: value, value: v };
        latest.current = next;
        setDraft(next);
        if (!preview) return;
        previewing.current = true;
        previewOps(() => ops(next.value));
      }}
      onCommit={(v, keyed) => {
        const committed = latest.current;
        previewing.current = false;
        if (keyed) {
          if (!burst.pending()) keyBase.current = value;
          burst.push({
            label: commitLabel,
            // Built when the burst commits, from the project as it is then.
            ops: () => ops(v),
            preview,
            noop: v === keyBase.current,
            done: () => settle(committed),
          });
          return;
        }
        burst.flush();
        // The new revision expires the dry run, so the frame never flickers back.
        if (preview) dropPendingPreview();
        // Through the commit queue, after the burst just flushed: built once a commit on its way has landed, and the
        // next edit (another control, undo, an export) waits for it.
        void burst
          .commit(commitLabel, () => ops(v))
          .then((ok) => {
            if (!ok && preview) clearPreview();
            // The project has the value now (or refused it): stop showing the draft, unless a newer drag owns it.
            settle(committed);
          });
      }}
      onBlur={burst.flush}
    />
  );
}
