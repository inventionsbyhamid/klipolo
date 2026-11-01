import type { RecorderSetup } from '../../bindings/RecorderSetup';
import type { WorkerState } from '../../bindings/WorkerState';

/**
 * What the Record dialog shows: `loading` while the setup is read; `unavailable` when it couldn't be (its sentence and
 * Try again); `choosing` the choices and Record (a failed take's sentence with them); `starting` from the Record press
 * through the countdown and the start gate (Cancel); `recording` and `saving` while the take runs and is saved, when
 * the dialog and Klipolo's window are hidden.
 */
export type RecorderPhase = 'loading' | 'unavailable' | 'choosing' | 'starting' | 'recording' | 'saving';

/** What the phase is read from: the dialog's setup and the take's worker, as the recorder store holds them. */
export interface PhaseInput {
  setup: RecorderSetup | null;
  setupError: string | null;
  /** The take's worker as its events say; `starting` from the Record press, before the take has an id. */
  worker: WorkerState | null;
}

/** Each worker state's phase; no worker (or one that ended or failed) is the choices. */
const BY_WORKER: Record<WorkerState, RecorderPhase> = {
  armed: 'starting',
  starting: 'starting',
  countdown: 'starting',
  checkingCamera: 'starting',
  recording: 'recording',
  paused: 'recording',
  stopping: 'saving',
  ready: 'saving',
  failed: 'choosing',
  idle: 'choosing',
};

export function phaseOf(s: PhaseInput): RecorderPhase {
  if (s.setupError !== null) return 'unavailable';
  if (s.setup === null) return 'loading';
  return s.worker === null ? 'choosing' : BY_WORKER[s.worker];
}
