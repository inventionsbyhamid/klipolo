import { memo, useLayoutEffect, useRef, type KeyboardEvent, type PointerEvent, type RefObject } from 'react';
import { stepFrames } from '../../../lib/time';
import { clockAnchor, clockUs, onClock } from '../../../state/playClock';
import { usePlayback } from '../../../state/playback';
import { startRangeDrag } from '../lanes/RangeSelection';
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
/** A press this close to the playhead's head (px) grabs it: dragging it scrubs. */
const HEAD_GRAB_PX = 7;

/** Where the playhead is drawn: the running clock while playing, else the stored time. */
function playheadNowUs(): number {
  const { playing, playheadUs } = usePlayback.getState();
  return playing && clockAnchor().running ? clockUs() : playheadUs;
}

/** How often the slider's value is said again while playing (ms): 10 times a second at most. */
const SPOKEN_MS = 100;

/** A time as a screen reader says it: "4 minutes 24.6 seconds", "26.4 seconds". */
export function spokenTime(us: number): string {
  const tenths = Math.floor(Math.max(0, us) / 100_000);
  const minutes = Math.floor(tenths / 600);
  const seconds = (tenths % 600) / 10;
  const secs = `${seconds} ${seconds === 1 ? 'second' : 'seconds'}`;
  if (minutes === 0) return secs;
  return `${minutes} ${minutes === 1 ? 'minute' : 'minutes'} ${secs}`;
}

/**
 * Keeps the slider's value and its words on the playhead outside React: on every seek, play and pause, and at most
 * 10 times a second while playing (never a render per frame).
 */
function useSliderValue(ref: RefObject<HTMLDivElement | null>, durationUs: number) {
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const say = () => {
      const at = Math.round(Math.min(Math.max(playheadNowUs(), 0), durationUs));
      el.setAttribute('aria-valuenow', String(at));
      el.setAttribute('aria-valuetext', spokenTime(at));
    };
    let timer: ReturnType<typeof setInterval> | undefined;
    const follow = () => {
      say();
      clearInterval(timer);
      timer = usePlayback.getState().playing ? setInterval(say, SPOKEN_MS) : undefined;
    };
    follow();
    const offClock = onClock(follow);
    const offStore = usePlayback.subscribe((s, prev) => {
      if (s.playing !== prev.playing || s.playheadUs !== prev.playheadUs) follow();
    });
    return () => {
      clearInterval(timer);
      offClock();
      offStore();
    };
  }, [ref, durationUs]);
}

interface RulerProps {
  width: number;
  pps: number;
  /** The project's frame rate, for the frame grid at the closest zooms and the arrow keys' frame steps. */
  fps: number;
  /** The video's length: the playhead slider's range. */
  durationUs: number;
  /** The window the lanes draw: the ruler draws its ticks only, a few screenfuls whatever the project's length. */
  viewport: Viewport;
  /** Moves the playhead to `px` from the start of the lanes. */
  onScrub: (px: number) => void;
}

/**
 * Time ruler: 24 px, labels at the major ticks, minor ticks between. A press moves the playhead there;
 * dragging from the playhead's head scrubs, and dragging anywhere else selects that time range
 * (`lanes/RangeSelection`). `memo`: an edit that keeps the lanes' width and window doesn't redraw its ticks
 * (`onScrub` must be stable).
 */
export const Ruler = memo(function Ruler({ width, pps, fps, durationUs, viewport, onScrub }: RulerProps) {
  const ref = useRef<HTMLDivElement>(null);
  useSliderValue(ref, durationUs);
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
  // The playhead's keys while the slider has focus: a frame (with Shift a second), the start and the end.
  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    const { seek } = usePlayback.getState();
    const at = playheadNowUs();
    const to =
      e.key === 'ArrowLeft' || e.key === 'ArrowRight'
        ? stepFrames(at, (e.key === 'ArrowLeft' ? -1 : 1) * (e.shiftKey ? Math.round(fps) : 1), fps, durationUs)
        : e.key === 'Home'
          ? 0
          : e.key === 'End'
            ? durationUs
            : null;
    if (to === null || durationUs <= 0) return;
    e.preventDefault();
    // The window's own shortcuts for these keys don't run as well.
    e.stopPropagation();
    seek(to);
  };
  const firstTick = times.length > 0 ? usToPx(times[0], pps) : 0;
  return (
    <div
      ref={ref}
      data-testid="ruler"
      className="tl-ruler"
      style={{ width }}
      // The playhead, as a slider: its value and words follow it outside React (`useSliderValue`).
      role="slider"
      tabIndex={durationUs > 0 ? 0 : -1}
      aria-label="Playhead"
      aria-valuemin={0}
      aria-valuemax={durationUs}
      aria-disabled={durationUs <= 0 || undefined}
      onKeyDown={onKeyDown}
      onPointerDown={(e) => {
        if (e.button !== 0) return;
        const x = e.clientX - e.currentTarget.getBoundingClientRect().left;
        const onHead = Math.abs(x - usToPx(playheadNowUs(), pps)) <= HEAD_GRAB_PX;
        // The head: dragging it scrubs. Elsewhere a drag selects a time range.
        e.currentTarget.dataset.scrubbing = String(onHead);
        if (onHead) e.currentTarget.setPointerCapture?.(e.pointerId);
        scrub(e);
        if (!onHead) startRangeDrag(e, e.currentTarget);
      }}
      onPointerMove={(e) => {
        if (e.currentTarget.dataset.scrubbing === 'true' && e.currentTarget.hasPointerCapture?.(e.pointerId)) scrub(e);
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
