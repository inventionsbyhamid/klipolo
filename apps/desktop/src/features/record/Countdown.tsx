import { type CSSProperties, useId } from 'react';
import { Button } from '../../design/Button';
import { Icon } from '../../layout/Icon';
import { useRecorder } from '../../state/recorder';
import { gateSentence } from './notes';

/** The meter's share of a peak: empty at -60 dBFS or below, full at 0 dBFS. */
export function meterShare(dbfs: number | null): number {
  if (dbfs === null || !Number.isFinite(dbfs)) return 0;
  return Math.min(1, Math.max(0, (dbfs + 60) / 60));
}

/** "Recording starts in 3 s.", as the countdown says it (and announces it, once). */
export const startsIn = (left: number) => `Recording starts in ${left} s.`;

/**
 * Over the preview's centre from the Record press: "Getting ready" while the take is arming (or waits for the last
 * one to go), then the countdown's number on a scrim circle. Nothing while the start gate asks.
 */
export function CountdownNumber() {
  const left = useRecorder((s) => s.countdown);
  const asking = useRecorder((s) => (s.gate?.length ?? 0) > 0);
  if (asking) return null;
  if (left === null)
    return (
      <p className="record-wait" role="status">
        <span className="spinner" aria-hidden />
        Getting ready
      </p>
    );
  return (
    <span className="record-count" aria-hidden>
      {left}
    </span>
  );
}

/**
 * Under the preview from the Record press: the countdown's sentence and the microphone's level, or the start gate's
 * question about each source that sends nothing yet ("Record without it"). Cancel is the footer's. What a press here
 * couldn't do is said too.
 */
export function Countdown() {
  const left = useRecorder((s) => s.countdown);
  const gate = useRecorder((s) => s.gate);
  const mic = useRecorder((s) => s.picks.mic !== null);
  const error = useRecorder((s) => s.error);
  const asking = gate !== null && gate.length > 0;
  return (
    <div className="record-countdown">
      {asking ? (
        <Gate missing={gate} />
      ) : (
        left !== null && (
          <>
            <p className="record-starts">{startsIn(left)}</p>
            {mic && <Meter />}
          </>
        )
      )}
      {error !== null && (
        <p className="record-said-line" role="alert">
          <Icon name="info" />
          {error}
        </p>
      )}
    </div>
  );
}

/** The microphone's level during the countdown: a 6 px track, its paper fill following the peaks. */
function Meter() {
  const dbfs = useRecorder((s) => s.meterDbfs);
  const share = meterShare(dbfs);
  return (
    <div
      className="record-meter"
      role="meter"
      aria-label="Microphone level"
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={Math.round(share * 100)}
      style={{ '--p': `${share * 100}%` } as CSSProperties}
    >
      <i />
    </div>
  );
}

/**
 * The countdown's start gate: a chosen source sent nothing yet, so the countdown waits. Each can be left
 * out with "Record without it", described by its sentence; the footer's Cancel lets the take go.
 */
function Gate({ missing }: { missing: string[] }) {
  const baseId = useId();
  const recordWithout = useRecorder((s) => s.recordWithout);
  return (
    <div className="record-gate" role="group" aria-labelledby={`${baseId}-0`}>
      {missing.map((source, i) => (
        <div key={source} className="record-gate-row">
          <p id={`${baseId}-${i}`}>{gateSentence(source)}</p>
          <Button
            size="sm"
            data-gate={i === 0 ? 'first' : undefined}
            aria-describedby={`${baseId}-${i}`}
            onClick={() => void recordWithout(source)}
          >
            Record without it
          </Button>
        </div>
      ))}
    </div>
  );
}
