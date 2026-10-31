import type { CSSProperties, KeyboardEvent, PointerEvent, ReactNode } from 'react';
import { IconButton } from '../design/IconButton';
import { Tooltip } from '../design/Tooltip';
import { useCommands } from '../features/command-palette/useCommands';
import { Timecode } from '../features/preview/Timecode';
import { useZoomTarget, zoomRange, zoomTo, zoomViewUs } from '../features/timeline/commands';
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
 * The timeline's zoom slider: drag or press it, or use the arrow keys (a step of 1, with Shift 10, out of
 * 100). Zooms around the playhead, as Command = and Command minus do; its words say how much of the video shows.
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
  const value = Math.round(share * 100);
  const viewUs = zoomViewUs();
  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (!range) return;
    const step = e.shiftKey ? 10 : 1;
    const to = { ArrowRight: value + step, ArrowUp: value + step, ArrowLeft: value - step, ArrowDown: value - step };
    const next = to[e.key as keyof typeof to];
    if (next === undefined) return;
    e.preventDefault();
    zoomTo(zoomAt(next / 100, range));
  };
  return (
    <div
      className="slider is-live"
      role="slider"
      tabIndex={range ? 0 : -1}
      aria-label="Timeline zoom"
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={value}
      aria-valuetext={viewUs === null ? undefined : `Shows ${formatTime(viewUs)}`}
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

/** A ghost button in the player bar, with its tooltip: what it does and its key, or why it is off. */
function BarButton({
  icon,
  label,
  tip,
  shortcut,
  enabled,
  onClick,
}: {
  icon: ReactNode;
  label: ReactNode;
  tip: string;
  shortcut?: string;
  enabled: boolean;
  onClick: () => void;
}) {
  return (
    <Tooltip label={tip} shortcut={enabled ? shortcut : undefined} side="top">
      <button
        type="button"
        className="btn btn-ghost btn-sm"
        // Off but focusable, so its tooltip says why.
        aria-disabled={!enabled || undefined}
        onClick={() => enabled && onClick()}
      >
        {icon}
        {label}
      </button>
    </Tooltip>
  );
}

/**
 * The player bar: Split and Delete on the left, the transport centred on the window, the timeline's zoom and Fit and
 * The volume on the right. Split, Delete, the zoom and Fit run the same commands as their keys; every
 * control says what it does and its key in a tooltip.
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
        <BarButton
          icon={<Icon name="scissors" />}
          label={
            <>
              Split<kbd>S</kbd>
            </>
          }
          tip={split?.enabled ? 'Split at the playhead' : 'Move the playhead over a clip to split it'}
          enabled={!!split?.enabled}
          onClick={() => run('split')}
        />
        <BarButton
          icon={<Icon name="trash" />}
          label={
            <>
              Delete<kbd>&#9003;</kbd>
            </>
          }
          tip={remove?.enabled ? remove.label : 'Select something first'}
          enabled={!!remove?.enabled}
          onClick={() => run('delete')}
        />
      </div>
      <div className="transport">
        <Timecode durationUs={duration} className="time time-now" />
        <IconButton
          icon={<Icon name="start" />}
          label="Go to start"
          shortcut="Home"
          disabled={!ready}
          onClick={() => usePlayback.getState().seek(0)}
        />
        <IconButton
          icon={<Icon name="back" />}
          label="Back 5 seconds"
          disabled={!ready}
          onClick={() => skip(-5_000_000, duration)}
        />
        <Tooltip label={playing ? 'Pause' : 'Play'} shortcut="Space" side="top">
          <button
            type="button"
            className="play"
            aria-label={playing ? 'Pause' : 'Play'}
            disabled={!ready}
            onClick={() => usePlayback.getState().toggle()}
          >
            {playing ? <Icon name="pause" /> : <Icon name="play" className="is-play" />}
          </button>
        </Tooltip>
        <IconButton
          icon={<Icon name="fwd" />}
          label="Forward 5 seconds"
          disabled={!ready}
          onClick={() => skip(5_000_000, duration)}
        />
        <IconButton icon={<Icon name="loop" />} label="Loop the selection" disabled />
        <span className="time time-total">{formatTime(duration)}</span>
      </div>
      <div className="pb-right">
        <div className="zoom">
          <IconButton
            icon={<Icon name="zoom-out" />}
            label="Zoom out"
            shortcut="⌘−"
            className="icon-btn-sm"
            disabled={!command('zoom-out')?.enabled}
            onClick={() => run('zoom-out')}
          />
          <ZoomSlider />
          <IconButton
            icon={<Icon name="zoom-in" />}
            label="Zoom in"
            shortcut="⌘="
            className="icon-btn-sm"
            disabled={!command('zoom-in')?.enabled}
            onClick={() => run('zoom-in')}
          />
        </div>
        <Tooltip label="Fit the whole video" shortcut="⇧Z" side="top">
          <button
            type="button"
            className="btn btn-ghost btn-sm"
            disabled={!command('zoom-fit')?.enabled}
            onClick={() => run('zoom-fit')}
          >
            Fit
          </button>
        </Tooltip>
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
