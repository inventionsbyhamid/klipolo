import type { Ref } from 'react';
import { cn } from '../../design/cn';

/**
 * The line a dragged edge snapped to: across the lanes (`lanes`), and its part in the ruler row (`ruler`), which the
 * sticky ruler row covers the lanes' part under. Hidden until a drag snaps; the drag moves both through their style
 * (`showGuide`), so they never re-render. Its x 0 is the lanes' start, `left` px into its container.
 */
export function SnapGuide({
  ref,
  part,
  left = 0,
}: {
  ref: Ref<HTMLDivElement>;
  part: 'lanes' | 'ruler';
  left?: number;
}) {
  return (
    <div
      ref={ref}
      aria-hidden
      data-snap-guide={part}
      // In the ruler row, over the playhead's head (z-index 2) as it is over the playhead's line in the lanes.
      className={cn('pointer-events-none absolute top-0 bottom-0 hidden w-px bg-signal-ink', part === 'ruler' && 'z-3')}
      style={{ left }}
    />
  );
}
