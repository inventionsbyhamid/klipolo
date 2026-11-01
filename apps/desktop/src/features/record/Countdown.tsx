import { useId } from 'react';
import { Button } from '../../design/Button';
import { Icon } from '../../layout/Icon';
import { useRecorder } from '../../state/recorder';
import { MicMeter } from './MicMeter';
import { gateSentence, systemAlertSentence } from './notes';

/** "Recording starts in 3 s.", as the countdown says it (and announces it, once). */
export const startsIn = (left: number) => `Recording starts in ${left} s.`;

/**
 * Over the preview's centre from the Record press: "Getting ready" while the take is arming (or waits for the last
 * one to go), then the countdown's number on a scrim circle. Nothing while the start gate or macOS asks.
 */
export function CountdownNumber() {
  const left = useRecorder((s) => s.countdown);
  const asking = useRecorder((s) => (s.gate?.length ?? 0) > 0 || s.systemAlert);
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
 * Under the preview from the Record press: the countdown's sentence and the microphone's level, or while macOS asks
 * something on the screen, that it waits for the answer (said as it comes), or the start gate's question about each
 * source that sends nothing yet ("Record without it"). Cancel is the footer's. What a press here couldn't do is said
 * too.
 */
export function Countdown() {
  const left = useRecorder((s) => s.countdown);
  const gate = useRecorder((s) => s.gate);
  const alert = useRecorder((s) => s.systemAlert);
  const mic = useRecorder((s) => s.picks.mic !== null);
  const error = useRecorder((s) => s.error);
  const asking = gate !== null && gate.length > 0;
  return (
    <div className="record-countdown">
      {alert ? (
        <p className="record-starts" role="status">
          {systemAlertSentence()}
        </p>
      ) : asking ? (
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

/** The microphone's level during the countdown, as the take's worker hears it. */
function Meter() {
  const dbfs = useRecorder((s) => s.meterDbfs);
  return <MicMeter dbfs={dbfs} />;
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
