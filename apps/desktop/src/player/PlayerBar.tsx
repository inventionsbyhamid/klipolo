import type { CSSProperties } from 'react';
import { Timecode } from '../features/preview/Timecode';
import { Icon } from '../layout/Icon';
import { useEngine } from '../state/engine';
import { usePlayback } from '../state/playback';
import { skip, useDurationUs } from '../state/transport';
import { formatTime } from './time';

/** The player bar: the transport centred on the window, with edit, zoom and volume placeholders on either side. */
export function PlayerBar() {
  const duration = useDurationUs();
  const playing = usePlayback((s) => s.playing);
  const ready = useEngine((s) => !!s.info && s.status !== 'unavailable') && duration > 0;
  return (
    <footer className="playerbar">
      <div className="pb-left">
        <button className="btn btn-ghost btn-sm" disabled>
          <Icon name="scissors" />
          Split<kbd>S</kbd>
        </button>
        <button className="btn btn-ghost btn-sm" disabled title="Select something first">
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
          title="Back 5 s (J)"
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
          title="Forward 5 s (L)"
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
        <div className="zoom" aria-hidden>
          <Icon name="zoom-out" />
          <div className="slider" style={{ '--v': '8%' } as CSSProperties}>
            <i />
          </div>
          <Icon name="zoom-in" />
        </div>
        <button className="btn btn-ghost btn-sm" disabled>
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
