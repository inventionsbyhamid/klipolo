import type { Ref } from 'react';

/**
 * The line a dragged edge snapped to, across the lanes. Hidden until a drag snaps; the drag moves it through its style
 * (`showGuide`), so it never re-renders.
 */
export function SnapGuide({ ref }: { ref: Ref<HTMLDivElement> }) {
  return (
    <div
      ref={ref}
      aria-hidden
      className="pointer-events-none absolute top-0 bottom-0 left-0 hidden w-px bg-signal-ink"
    />
  );
}
