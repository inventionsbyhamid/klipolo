import type { CSSProperties, KeyboardEvent, PointerEvent } from 'react';
import { IconButton } from '../design/IconButton';
import { Icon } from '../layout/Icon';
import { useVolume } from '../state/volume';

/**
 * The player bar's volume: Mute, then a slider. It sets how loud the preview plays for the person, never
 * what an export holds. Drag or press the slider, or use the arrow keys (a step of 5, with Shift 20, out of 100; Home
 * and End the ends); moving it unmutes. While muted the slider shows 0, and Unmute (or an arrow key's step from it)
 * brings back the level it had.
 */
export function VolumeControl() {
  const level = useVolume((s) => s.level);
  const muted = useVolume((s) => s.muted);
  const shown = muted ? 0 : level;
  const value = Math.round(shown * 100);
  const setLevel = (next: number) => useVolume.getState().setLevel(next);
  const fromPointer = (e: PointerEvent<HTMLDivElement>) => {
    const box = e.currentTarget.getBoundingClientRect();
    if (box.width > 0) setLevel((e.clientX - box.left) / box.width);
  };
  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    const step = e.shiftKey ? 20 : 5;
    // Muted, a step goes from the level Unmute would bring back.
    const from = Math.round(level * 100);
    const to: Record<string, number> = {
      ArrowRight: from + step,
      ArrowUp: from + step,
      ArrowLeft: from - step,
      ArrowDown: from - step,
      Home: 0,
      End: 100,
    };
    const next = to[e.key];
    if (next === undefined) return;
    e.preventDefault();
    setLevel(next / 100);
  };
  return (
    <div className="zoom vol">
      <IconButton
        icon={<Icon name={shown === 0 ? 'volume-off' : 'volume'} />}
        label={muted ? 'Unmute' : 'Mute'}
        tooltip={muted ? 'Unmute the preview' : 'Mute the preview'}
        className="icon-btn-sm"
        onClick={() => useVolume.getState().toggleMute()}
      />
      <div
        className="slider is-live"
        role="slider"
        tabIndex={0}
        aria-label="Preview volume"
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={value}
        aria-valuetext={muted ? 'Muted' : `${value}%`}
        style={{ '--v': `${shown * 100}%` } as CSSProperties}
        onPointerDown={(e) => {
          e.currentTarget.setPointerCapture?.(e.pointerId);
          fromPointer(e);
        }}
        onPointerMove={(e) => {
          if (e.currentTarget.hasPointerCapture?.(e.pointerId)) fromPointer(e);
        }}
        onKeyDown={onKeyDown}
      >
        <i />
      </div>
    </div>
  );
}
