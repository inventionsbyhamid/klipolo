import type { CSSProperties, KeyboardEvent, PointerEvent } from 'react';
import type { Command } from '../features/command-palette/command';
import { useCommands } from '../features/command-palette/useCommands';
import { Timecode } from '../features/preview/Timecode';
import { useZoomTarget, zoomRange, zoomTo } from '../features/timeline/commands';
import { Icon } from '../layout/Icon';
import { useEngine } from '../state/engine';
import { usePlayback } from '../state/playback';
import { skip, useDurationUs } from '../state/transport';
import { useUi } from '../state/ui';
import { formatTime } from './time';

/** The zoom as the slider shows it: 0 (the whole video) to 1 (the closest), on a log scale. */
function zoomShare(pps: number, range: [number, number] | null): number {
  if (!range) return 0;
  const [min, max] = range;
  if (max <= min) return 0;
  return Math.min(1, Math.max(0, Math.log(pps / min) / Math.log(max / min)));
}

function zoomAt(share: number, range: [number, number]): number {
  const [min, max] = range;
  return min * (max / min) ** Math.min(1, Math.max(0, share));
}

/**
 * The timeline's zoom slider: drag or press it, or use the arrow keys (Shift for bigger steps). Zooms
 * around the playhead, as Command = and Command minus do.
 */
function ZoomSlider() {
  const pps = useUi((s) => s.pxPerSecond);
  // Read again when a timeline comes or goes, and when the video's length changes the farthest zoom.
  useZoomTarget((s) => s.version);
  useDurationUs();
  const range = zoomRange();
  const share = zoomShare(pps, range);
  const fromPointer = (e: PointerEvent<HTMLDivElement>) => {
    if (!range) return;
    const box = e.currentTarget.getBoundingClientRect();
    if (box.width > 0) zoomTo(zoomAt((e.clientX - box.left) / box.width, range));
  };
  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (!range) return;
    const step = e.shiftKey ? 0.1 : 0.02;
    const to = { ArrowRight: share + step, ArrowUp: share + step, ArrowLeft: share - step, ArrowDown: share - step };
    const next = to[e.key as keyof typeof to];
    if (next === undefined) return;
    e.preventDefault();
    zoomTo(zoomAt(next, range));
  };
  return (
    <div
      className="slider is-live"
      role="slider"
      tabIndex={range ? 0 : -1}
      aria-label="Timeline zoom"
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={Math.round(share * 100)}
      aria-disabled={!range}
      style={{ '--v': `${share * 100}%` } as CSSProperties}
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
  );
}

/** A command's tooltip: its name and its key. */
const tip = (command: Command | undefined, fallback: string) =>
  command ? `${command.label} (${typeof command.shortcut === 'string' ? command.shortcut : ''})` : fallback;

/**
 * The player bar: Split and Delete on the left, the transport centred on the window, the timeline's zoom and Fit and
 * The volume on the right. Split, Delete, the zoom and Fit run the same commands as their keys.
 */
export function PlayerBar() {
  const duration = useDurationUs();
  const playing = usePlayback((s) => s.playing);
  const ready = useEngine((s) => !!s.info && s.status !== 'unavailable') && duration > 0;
  const commands = useCommands();
  const command = (id: string) => commands.find((c) => c.id === id);
  const split = command('split');
  const remove = command('delete');
  const run = (id: string) => {
    const c = command(id);
    if (c?.enabled) c.run();
  };
  return (
    <footer className="playerbar">
      <div className="pb-left">
        <button
          className="btn btn-ghost btn-sm"
          disabled={!split?.enabled}
          title={split?.enabled ? 'Split at the playhead (S)' : 'Move the playhead over a clip to split it'}
          onClick={() => run('split')}
        >
          <Icon name="scissors" />
          Split<kbd>S</kbd>
        </button>
        <button
          className="btn btn-ghost btn-sm"
          disabled={!remove?.enabled}
          title={remove?.enabled ? `${remove.label} (Delete)` : 'Select something first'}
          onClick={() => run('delete')}
        >
          <Icon name="trash" />
          Delete<kbd>&#9003;</kbd>
        </button>
      </div>
      <div className="transport">
        <Timecode durationUs={duration} className="time time-now" />
        <button
          className="icon-btn"
          aria-label="Go to start"
          title="Go to start (Home)"
          disabled={!ready}
          onClick={() => usePlayback.getState().seek(0)}
        >
          <Icon name="start" />
        </button>
        <button
          className="icon-btn"
          aria-label="Back 5 seconds"
          title="Back 5 s"
          disabled={!ready}
          onClick={() => skip(-5_000_000, duration)}
        >
          <Icon name="back" />
        </button>
        <button
          className="play"
          aria-label={playing ? 'Pause' : 'Play'}
          title={playing ? 'Pause (Space)' : 'Play (Space)'}
          disabled={!ready}
          onClick={() => usePlayback.getState().toggle()}
        >
          {playing ? <Icon name="pause" /> : <Icon name="play" className="is-play" />}
        </button>
        <button
          className="icon-btn"
          aria-label="Forward 5 seconds"
          title="Forward 5 s"
          disabled={!ready}
          onClick={() => skip(5_000_000, duration)}
        >
          <Icon name="fwd" />
        </button>
        <button className="icon-btn" aria-label="Loop the selection" title="Loop the selection" disabled>
          <Icon name="loop" />
        </button>
        <span className="time time-total">{formatTime(duration)}</span>
      </div>
      <div className="pb-right">
        <div className="zoom">
          <button
            className="icon-btn icon-btn-sm"
            aria-label="Zoom out"
            title={tip(command('zoom-out'), 'Zoom out')}
            disabled={!command('zoom-out')?.enabled}
            onClick={() => run('zoom-out')}
          >
            <Icon name="zoom-out" />
          </button>
          <ZoomSlider />
          <button
            className="icon-btn icon-btn-sm"
            aria-label="Zoom in"
            title={tip(command('zoom-in'), 'Zoom in')}
            disabled={!command('zoom-in')?.enabled}
            onClick={() => run('zoom-in')}
          >
            <Icon name="zoom-in" />
          </button>
        </div>
        <button
          className="btn btn-ghost btn-sm"
          title="Fit the whole video (Shift Z)"
          disabled={!command('zoom-fit')?.enabled}
          onClick={() => run('zoom-fit')}
        >
          Fit
        </button>
        <div className="zoom vol" aria-hidden>
          <Icon name="volume" />
          <div className="slider" style={{ '--v': '80%' } as CSSProperties}>
            <i />
          </div>
        </div>
      </div>
    </footer>
  );
}
