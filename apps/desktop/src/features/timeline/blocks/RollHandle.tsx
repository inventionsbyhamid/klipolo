import { useLingui } from '@lingui/react/macro';
import { memo, useCallback, type PointerEvent } from 'react';
import { registerBlock, rollId } from '../drag/registry';
import { onBlockLostCapture, onBlockPointerCancel, onBlockPointerMove, onBlockPointerUp } from '../drag/useBlockDrag';
import { usToPx } from '../scale';

/** A lane's press handler for its roll handles: stable, so the `memo` handles never re-render for it. */
export type PressRoll = (e: PointerEvent<HTMLElement>, clipId: string) => void;

interface RollHandleProps {
  /** The clip before the cut. */
  clipId: string;
  cutUs: number;
  pps: number;
  onRollPress: PressRoll;
}

/**
 * The cut between two clips, as a thin strip a drag rolls (`drag/editFrame`): the clip before it ends later or earlier
 * and the one after starts with it, so the video keeps its length. Its cursor says so on hover. It sits over the two
 * clips' trim pads, between them (each clip's own edge just inside it still trims that clip), and above the cut's
 * marker along the lane's bottom; the lane's one tip (`CutTip`) names it. Pointer only: ⌥, and ⌥. roll the cut at the
 * playhead.
 */
export const RollHandle = memo(function RollHandle({ clipId, cutUs, pps, onRollPress }: RollHandleProps) {
  const { t } = useLingui();
  const ref = useCallback((el: HTMLElement | null) => registerBlock(rollId(clipId), el), [clipId]);
  return (
    <span
      ref={ref}
      aria-hidden
      data-roll={clipId}
      data-tip={t({ id: 'timeline.roll.handle', message: 'Drag to roll this cut' })}
      className="tl-roll"
      style={{ left: usToPx(cutUs, pps) }}
      onPointerDown={(e) => onRollPress(e, clipId)}
      onPointerMove={onBlockPointerMove}
      onPointerUp={onBlockPointerUp}
      onPointerCancel={onBlockPointerCancel}
      onLostPointerCapture={onBlockLostCapture}
    />
  );
});
