import { useLingui } from '@lingui/react/macro';
import { Minus, Plus } from 'lucide-react';
import { useState } from 'react';
import type { Op } from '../../bindings/Op';
import { IconButton } from '../../design/IconButton';
import { useBurst } from '../../state/useBurst';
import { ToolSlider } from './ToolSlider';

/** The size slider's range: up to the largest size the model takes (`model.text.size`, 4 to 400); under 8 none reads. */
export const MIN_TEXT_SIZE = 8;
export const MAX_TEXT_SIZE = 400;

/** Sizes the − and + buttons step through (pixels at a 1080-pixel short side). */
export const SIZE_STEPS = [
  8, 12, 16, 20, 24, 28, 32, 40, 48, 56, 64, 72, 80, 96, 112, 128, 144, 160, 192, 224, 256, 320, 400,
];

/** The size after `size` going `dir`: the next step that way, or `size` itself at the end. */
export function stepSize(size: number, dir: 1 | -1): number {
  const next = dir > 0 ? SIZE_STEPS.find((s) => s > size) : [...SIZE_STEPS].reverse().find((s) => s < size);
  return next ?? size;
}

/**
 * A text's Size: the slider, with − and + on either side stepping through common sizes. Quick presses build on each
 * other, shown at once, and are one undo step once they stop (`useBurst`); the outline grows with the text (`ops`).
 */
export function TextSizeField({ size, ops }: { size: number; ops: (size: number) => Op[] }) {
  const { t } = useLingui();
  /** A size not yet in the project, so quick presses build on each other and commit as one undo step. */
  const [pending, setPending] = useState<number | null>(null);
  const burst = useBurst();
  const shown = pending ?? size;
  const sizeLabel = t({ id: 'text.size', message: 'Size' });

  const resize = (dir: 1 | -1) => {
    const next = stepSize(shown, dir);
    if (next === shown) return;
    setPending(next);
    burst.push({
      label: 'text.size.undo',
      // Built at its turn, from the title as the project has it then.
      ops: () => ops(next),
      noop: next === size,
      done: () => setPending((p) => (p === next ? null : p)),
    });
  };

  return (
    <div className="text-size-row">
      <IconButton
        icon={Minus}
        className="icon-btn-sm"
        label={t({ id: 'text.size.smaller', message: 'Smaller text' })}
        aria-disabled={shown <= SIZE_STEPS[0] || undefined}
        onClick={() => resize(-1)}
        onBlur={burst.flush}
      />
      <ToolSlider
        label={sizeLabel}
        commitLabel="text.size.undo"
        value={shown}
        min={MIN_TEXT_SIZE}
        max={MAX_TEXT_SIZE}
        step={1}
        format={(v) => String(Math.round(v))}
        ops={ops}
      />
      <IconButton
        icon={Plus}
        className="icon-btn-sm"
        label={t({ id: 'text.size.larger', message: 'Larger text' })}
        aria-disabled={shown >= SIZE_STEPS[SIZE_STEPS.length - 1] || undefined}
        onClick={() => resize(1)}
        onBlur={burst.flush}
      />
    </div>
  );
}
