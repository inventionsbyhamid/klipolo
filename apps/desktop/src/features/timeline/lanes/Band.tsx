import { memo } from 'react';
import { cn } from '../../../design/cn';

interface BandProps {
  left: number;
  width: number;
  top?: number;
  height?: number;
  look: BandLook;
  /** Several bands drawn as one: the CSS mask that paints only their parts (`bandMask`), leaving the gaps empty. */
  mask?: string;
}

/** The lane a band is on, which gives it its colour (`.tl-band.is-*` in app.css). */
export type BandLook = 'video' | 'overlay' | 'sound' | 'music';

/**
 * Blocks too narrow to draw one by one (an hour zoomed out), drawn as one strip, or a row's run of such strips as one
 * element masked to their parts. Lanes draw bands before their blocks, so a block (a kept tiny one drawn wider than
 * its time) is always on top; clicks go through to the lane, which seeks, and never reach or deselect a block.
 */
export const Band = memo(function Band({ left, width, top, height, look, mask }: BandProps) {
  return (
    <span
      data-band
      aria-hidden
      className={cn('tl-band', `is-${look}`, mask === undefined && 'is-round')}
      style={{ left, width: Math.max(1, width), top, height, maskImage: mask, WebkitMaskImage: mask }}
    />
  );
});

/* eslint-disable lingui/no-unlocalized-strings, no-restricted-syntax -- CSS values */
/**
 * The mask that paints `parts` (px from the band's left, each at least 1 px wide) and nothing between them; undefined
 * for one part (the band is that part).
 */
export function bandMask(parts: readonly (readonly [number, number])[]): string | undefined {
  if (parts.length < 2) return undefined;
  const stops: string[] = [];
  let at = 0;
  for (const [from, to] of parts) {
    const a = Math.max(at, from);
    const b = Math.max(a + 1, to);
    if (a > at) stops.push(`transparent ${at.toFixed(1)}px ${a.toFixed(1)}px`);
    stops.push(`#000 ${a.toFixed(1)}px ${b.toFixed(1)}px`);
    at = b;
  }
  stops.push(`transparent ${at.toFixed(1)}px`);
  return `linear-gradient(to right, ${stops.join(', ')})`;
}
/* eslint-enable lingui/no-unlocalized-strings, no-restricted-syntax */
