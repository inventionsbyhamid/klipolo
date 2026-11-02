import { useId } from 'react';
import type { RecorderSetup } from '../../bindings/RecorderSetup';
import { Button } from '../../design/Button';
import { Icon } from '../../layout/Icon';
import { type ScreenStage, screenStage } from '../../lib/recorder/permission';
import { useRecorder } from '../../state/recorder';
import { RecordIcon } from './icons';

/** The step's button: a press on Record while the step shows puts the keyboard on it. */
export const STEP_BUTTON_ID = 'record-screen-step';

const SENTENCE: Record<ScreenStage, string> = {
  ask: 'Klipolo needs your permission to record your screen.',
  reopen: 'Turn on Klipolo in the list that opened, then reopen Klipolo.',
  stillRefused:
    "macOS still isn't letting Klipolo record the screen. This can happen when an older copy of Klipolo is in the list.",
};

const REMOVE_OLD =
  'Open Screen & System Audio Recording, select Klipolo, remove it with the minus button, then press Allow again.';

/**
 * macOS's Screen Recording step, in the preview's place while the take needs the screen and Klipolo
 * may not record it yet. "Allow screen recording" asks macOS (on this press alone) and opens its settings page; then
 * "Reopen Klipolo", as macOS applies the grant only to a Klipolo started after it. When macOS still says no after that,
 * the step says how to remove the older copy's entry instead of asking in a loop. Its button is the one red button,
 * and the same element through every stage, so the keyboard stays on it as Allow turns into Reopen. A refused press is
 * said in the step.
 */
export function ScreenStep({ setup }: { setup: RecorderSetup }) {
  const asked = useRecorder((s) => s.askedScreen);
  const error = useRecorder((s) => s.error);
  const textId = useId();
  const stepsId = useId();
  const stage = screenStage(setup, asked);
  const still = stage === 'stillRefused';
  const { allowScreen, reopen, openPrivacy } = useRecorder.getState();
  return (
    <div className="record-step">
      <p id={textId}>
        <Icon name="info" />
        {SENTENCE[stage]}
      </p>
      {still && (
        <p className="record-step-more" id={stepsId}>
          {REMOVE_OLD}
        </p>
      )}
      {error !== null && (
        <p className="record-step-said" role="alert">
          {error}
        </p>
      )}
      <div className="record-actions">
        {stage !== 'ask' && setup.privacySettings && (
          <Button key="settings" variant="secondary" onClick={() => void openPrivacy('screen')}>
            Open privacy settings
          </Button>
        )}
        <Button
          key="step"
          id={STEP_BUTTON_ID}
          variant="primary"
          aria-describedby={still ? `${textId} ${stepsId}` : textId}
          onClick={() => void (stage === 'reopen' ? reopen() : allowScreen())}
        >
          {stage === 'reopen' ? 'Reopen Klipolo' : 'Allow screen recording'}
        </Button>
      </div>
    </div>
  );
}

const DENIED = {
  mic: "Klipolo can't use your microphone. Allow it in Privacy & Security, then try again.",
  camera: "Klipolo can't use your camera. Allow it in Privacy & Security, then try again.",
} as const;

/**
 * A microphone or camera macOS refused, while it is chosen: the alert icon, one sentence and "Open privacy
 * settings" where the system has that page. "No microphone" and "No camera" still record.
 */
export function DeniedNote({ kind, pages }: { kind: 'mic' | 'camera'; pages: boolean }) {
  const id = useId();
  return (
    <div className="record-denied">
      <p id={id}>
        <RecordIcon name="alert" />
        {DENIED[kind]}
      </p>
      {pages && (
        <Button
          variant="ghost"
          size="sm"
          aria-describedby={id}
          onClick={() => void useRecorder.getState().openPrivacy(kind)}
        >
          Open privacy settings
        </Button>
      )}
    </div>
  );
}
