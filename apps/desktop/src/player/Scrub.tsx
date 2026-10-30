import { useRef, type CSSProperties, type KeyboardEvent, type PointerEvent, type ReactNode } from 'react';
import { durationUs, seek, skip, useSession } from './session';
import { formatTime } from './time';
import { usePlayhead } from './usePlayhead';

/**
 * The timeline's scrub area (the ruler and the lanes): click or drag to move the playhead, arrow keys step a second.
 * `children` draw under the playhead.
 */
export function Scrub({ children }: { children?: ReactNode }) {
  const duration = useSession((s) => durationUs(s));
  const ready = useSession((s) => !!s.info) && duration > 0;
  const at = usePlayhead();
  const ref = useRef<HTMLDivElement>(null);
  const dragging = useRef(false);
  const pending = useRef<number | null>(null);

  const toUs = (clientX: number) => {
    const r = ref.current!.getBoundingClientRect();
    return Math.min(Math.max((clientX - r.left) / r.width, 0), 1) * duration;
  };
  // At most one seek per animation frame while dragging: the newest wins.
  const seekSoon = (us: number) => {
    const first = pending.current === null;
    pending.current = us;
    if (!first) return;
    requestAnimationFrame(() => {
      const to = pending.current;
      pending.current = null;
      if (to !== null) void seek(to);
    });
  };
  const onPointerDown = (e: PointerEvent<HTMLDivElement>) => {
    if (!ready || e.button !== 0) return;
    dragging.current = true;
    e.currentTarget.setPointerCapture(e.pointerId);
    seekSoon(toUs(e.clientX));
  };
  const onPointerMove = (e: PointerEvent<HTMLDivElement>) => {
    if (dragging.current) seekSoon(toUs(e.clientX));
  };
  const onPointerUp = () => {
    dragging.current = false;
  };
  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (!ready) return;
    if (e.key === 'ArrowLeft' || e.key === 'ArrowRight') {
      e.preventDefault();
      e.stopPropagation();
      void skip(e.key === 'ArrowLeft' ? -1_000_000 : 1_000_000);
    }
  };
  const x = duration > 0 ? `${(Math.min(at, duration) / duration) * 100}%` : '0%';
  return (
    <div
      ref={ref}
      className="tl-marks"
      role="slider"
      tabIndex={ready ? 0 : -1}
      aria-label="Playhead"
      aria-disabled={!ready}
      aria-valuemin={0}
      aria-valuemax={duration}
      aria-valuenow={Math.round(at)}
      aria-valuetext={formatTime(at)}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onPointerCancel={onPointerUp}
      onKeyDown={onKeyDown}
    >
      {children}
      {ready && <span className="playhead" style={{ '--x': x } as CSSProperties} />}
    </div>
  );
}
