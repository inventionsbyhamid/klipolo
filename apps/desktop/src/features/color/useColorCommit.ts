import { useEffect, useRef, useState } from 'react';
import type { Op } from '../../bindings/Op';
import { sameColor } from '../../design/color/colorMath';
import type { Label } from '../../lib/labels';
import { clearPreview, previewOps } from '../../state/livePreview';
import { useBurst } from '../../state/useBurst';

/** A colour being picked, shown while the project catches up. */
interface Draft {
  /** The setting's colour when it was picked: once the project has moved on from it, the draft is stale. */
  base: string | null;
  color: string | null;
}

interface ColorCommit {
  value: string | null;
  onPreview(color: string | null): void;
  onPreviewEnd(): void;
  onCommit(color: string | null): void;
}

/**
 * A colour setting's dry runs and undo steps, for `ColorField`, so every colour behaves the same. A hovered colour is
 * dry-run in the engine (once a frame); leaving puts the video back, but only while a dry run is showing (after a pick,
 * the new revision replaces it, so a clear would flash the old colour). A pick is one undo step through `useBurst`'s
 * commit queue, built when it goes in, and shown at once (`value` is the pick until the project has it); picking the
 * colour it has commits nothing. Spread the result onto `ColorField`.
 */
export function useColorCommit(o: {
  value: string | null;
  label: Label | ((color: string | null) => Label);
  ops: (color: string | null) => Op[];
}): ColorCommit {
  const { value, label, ops } = o;
  const burst = useBurst();
  const previewing = useRef(false);
  const [draft, setDraft] = useState<Draft | null>(null);
  // Gone mid-preview (the selection moved on): put the video back.
  useEffect(
    () => () => {
      if (!previewing.current) return;
      previewing.current = false;
      clearPreview();
    },
    [],
  );

  const onPreview = (color: string | null) => {
    previewing.current = true;
    previewOps(() => ops(color));
  };
  const onPreviewEnd = () => {
    if (!previewing.current) return;
    previewing.current = false;
    clearPreview();
  };
  const onCommit = (color: string | null) => {
    previewing.current = false;
    const next: Draft = { base: value, color };
    setDraft(next);
    burst.push({
      label: typeof label === 'function' ? label(color) : label,
      // Built when it goes in, from the project as it is then.
      ops: () => ops(color),
      noop: sameColor(color, value),
      done: () => setDraft((d) => (d === next ? null : d)),
    });
    burst.flush();
  };
  return { value: draft && draft.base === value ? draft.color : value, onPreview, onPreviewEnd, onCommit };
}
