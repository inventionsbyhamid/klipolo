import { memo, type PointerEvent } from 'react';
import { Playhead } from '../Playhead';
import { pxToUs, usToPx } from '../scale';
import type { Viewport } from '../useViewport';
import { frameGridPx, tickLabel, ticks } from './ticks';

/** Room a tick label needs (the widest, "1:00:00" or "0:00.5", plus its padding), so the last one isn't cut off. */
const LABEL_PX = 44;
/** The frame grid's lines. */
const LINE = 'var(--color-line-strong)';
/** Minor ticks between two labelled ones. */
const MINOR_PER_MAJOR = 5;


interface RulerProps {
  width: number;
  pps: number;
  /** The project's frame rate, for the frame grid at the closest zooms. */
  fps: number;
  /** The window the lanes draw: the ruler draws its ticks only, a few screenfuls whatever the project's length. */
  viewport: Viewport;
  /** Moves the playhead to `px` from the start of the lanes. */
  onScrub: (px: number) => void;
}

/**
 * Time ruler: 24 px, labels at the major ticks, minor ticks between. Press or drag on it to move the
 * playhead (selection is kept). `memo`: an edit that keeps the lanes' width and window doesn't redraw its ticks
 * (`onScrub` must be stable).
 */
export const Ruler = memo(function Ruler({ width, pps, fps, viewport, onScrub }: RulerProps) {
  // Never more than three views of ticks, even for one render with a window that hasn't caught up.
  const windowToUs = Math.min(viewport.toUs, viewport.fromUs + pxToUs(3 * viewport.widthPx, pps));
  const { step, times } = ticks(viewport.fromUs, Math.min(windowToUs, pxToUs(width - LABEL_PX, pps)), pps);
  // At the closest zooms, a line per frame across the window's width: one strip with a repeating gradient (exact at
  // 29.97 fps, where a tiled background would round), not an element per frame.
  const framePx = frameGridPx(pps, fps);
  const gridLeft = framePx ? Math.ceil((viewport.fromUs * fps) / 1_000_000) * framePx : 0;
  const gridWidth = framePx ? Math.min(usToPx(windowToUs, pps), width) - gridLeft : 0;
  const scrub = (e: PointerEvent<HTMLDivElement>) =>
    onScrub(e.clientX - e.currentTarget.getBoundingClientRect().left);
  const minorPx = (step * pps) / MINOR_PER_MAJOR;
  const firstTick = times.length > 0 ? usToPx(times[0], pps) : 0;
  return (
    <div
      data-testid="ruler"
      className="tl-ruler"
      style={{ width }}
      onPointerDown={(e) => {
        if (e.button !== 0) return;
        e.currentTarget.setPointerCapture?.(e.pointerId);
        scrub(e);
      }}
      onPointerMove={(e) => {
        if (e.currentTarget.hasPointerCapture?.(e.pointerId)) scrub(e);
      }}
    >
      {times.length > 0 && minorPx >= 4 && (
        <span
          aria-hidden
          className="tl-minor"
          style={{
            left: Math.max(0, firstTick - usToPx(step * 1e6, pps)),
            width: Math.min(width, usToPx(windowToUs, pps)) - Math.max(0, firstTick - usToPx(step * 1e6, pps)),
            backgroundSize: `${minorPx}px 5px`,
          }}
        />
      )}
      {framePx !== null && gridWidth > 0 && (
        <span
          aria-hidden
          data-frame-grid
          className="tl-frames"
          style={{
            left: gridLeft,
            width: gridWidth,
            backgroundImage: `repeating-linear-gradient(to right, ${LINE} 0 1px, transparent 1px ${framePx}px)`,
          }}
        />
      )}
      {times.map((us) => (
        <span
          key={us}
          aria-hidden
          data-tick
          className="tl-tick"
          style={{ left: usToPx(us, pps) }}
        >
          {tickLabel(us, step)}
        </span>
      ))}
      <Playhead pps={pps} head />
    </div>
  );
});
