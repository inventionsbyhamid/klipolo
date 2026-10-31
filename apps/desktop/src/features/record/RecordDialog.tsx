import { type KeyboardEvent, type ReactNode, useEffect, useRef, useState } from 'react';
import type { RecorderSetup } from '../../bindings/RecorderSetup';
import { codedMessage } from '../../bridge/call';
import { Button } from '../../design/Button';
import { Toggle } from '../../design/Toggle';
import { Tooltip } from '../../design/Tooltip';
import { Icon } from '../../layout/Icon';
import { deviceDenied, screenStep } from '../../lib/recorder/permission';
import { phaseOf, type RecorderPhase } from '../../lib/recorder/phase';
import { useRecorder } from '../../state/recorder';
import { DeviceSelect } from './DeviceSelect';
import { RecordIcon } from './icons';
import { Countdown, CountdownNumber, startsIn } from './Countdown';
import { cameraNote, notesBefore, notReady, refusedPage } from './notes';
import { DeniedNote, MicAskedAtRecord, ScreenStep, STEP_BUTTON_ID } from './PermissionStep';
import { WhatToRecord } from './WhatToRecord';
import './record.css';

/** What Tab moves between: enabled buttons in the Tab order (a radio group's other radios are left out). */
const FOCUSABLE = 'button:not(:disabled):not([tabindex="-1"]), [tabindex="0"]';

const STATUS = 'Saved in Movies / Klipolo / Recordings. Stop with Option Command R or from the menu bar.';

/** The keyboard is in an open select's menu, which the dialog's own keys leave alone (it is drawn outside it). */
const inMenu = (target: EventTarget | null) => target instanceof Element && target.closest('[role="menu"]') !== null;

/**
 * The Record dialog: what to record, the microphone and the camera, then Record. Mounted once; it shows
 * while the recorder store's dialog is open, and hides while its take records or saves, as Klipolo's window is hidden
 * then (nothing here can stop that take). Esc and the close button close it while choosing; from the Record press
 * through the countdown Esc cancels the take and the dialog stays, and the close button cancels it and closes.
 */
export function RecordDialog() {
  const open = useRecorder((s) => s.open);
  const phase = useRecorder(phaseOf);
  return open && phase !== 'recording' && phase !== 'saving' ? <Dialog phase={phase} /> : null;
}

/** Esc: cancels a take on its way, else closes. */
function onEscape() {
  const s = useRecorder.getState();
  if (phaseOf(s) === 'starting') void s.cancel();
  else s.close();
}

function Dialog({ phase }: { phase: Exclude<RecorderPhase, 'recording' | 'saving'> }) {
  const root = useRef<HTMLDivElement>(null);
  const title = useRef<HTMLHeadingElement>(null);
  const close = useRecorder((s) => s.close);
  const left = useRecorder((s) => s.countdown);
  const asking = useRecorder((s) => (s.gate?.length ?? 0) > 0);
  const gateKey = useRecorder((s) => s.gate?.join() ?? '');
  /** What the countdown announced: its first number, once per countdown. */
  const [announced, setAnnounced] = useState('');
  /** The sentence a take that didn't start came back with: Try again presses Record again for it. */
  const [takeError, setTakeError] = useState<string | null>(null);
  const was = useRef(phase);

  useEffect(() => {
    const dialog = root.current;
    const trigger = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    // The title first, never Record: an Enter meant for something else can't start a take.
    title.current?.focus();
    // The dialog is modal: no key reaches the editor behind it, and focus that leaves it comes back. An open select's
    // menu is drawn outside it and keeps its own keys (Esc closes the menu, not the dialog).
    const onKey = (e: globalThis.KeyboardEvent) => {
      if (inMenu(e.target)) return;
      if (e.key === 'Escape') {
        e.preventDefault();
        e.stopPropagation();
        onEscape();
      } else if (dialog && !dialog.contains(e.target as Node)) {
        e.preventDefault();
        e.stopPropagation();
        title.current?.focus();
      }
    };
    const onFocus = (e: FocusEvent) => {
      if (dialog && !dialog.contains(e.target as Node) && !inMenu(e.target)) title.current?.focus();
    };
    window.addEventListener('keydown', onKey, true);
    document.addEventListener('focusin', onFocus);
    return () => {
      window.removeEventListener('keydown', onKey, true);
      document.removeEventListener('focusin', onFocus);
      trigger?.focus();
    };
  }, []);

  // A take that ends before it starts (cancelled, or it failed) brings the choices back with the keyboard on the title,
  // never on Record, so an Enter pressed twice to be sure of Cancel can't record. Any other view that goes takes the
  // focused control with it: the keyboard goes to the title too.
  useEffect(() => {
    const before = was.current;
    was.current = phase;
    const dialog = root.current;
    if (before === 'starting' && phase === 'choosing') {
      setTakeError(useRecorder.getState().error);
      title.current?.focus();
    } else if (dialog && !dialog.contains(document.activeElement)) title.current?.focus();
  }, [phase]);

  // From the Record press the keyboard is on Cancel, and on the start gate's first answer while it asks.
  useEffect(() => {
    if (phase !== 'starting') return;
    const target = asking ? '[data-gate="first"]' : '[data-take-cancel]';
    root.current?.querySelector<HTMLElement>(target)?.focus();
  }, [phase, asking, gateKey]);

  // The countdown is announced once, by its first number (again once the start gate has been answered).
  useEffect(() => {
    if (phase !== 'starting' || asking) setAnnounced('');
    else if (left !== null) setAnnounced((said) => said || startsIn(left));
  }, [phase, asking, left]);

  // Keys typed in the dialog stay in it (the editor's shortcuts listen on the window).
  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    e.stopPropagation();
    if (e.key === 'Tab') trapFocus(e, root.current);
  };

  return (
    <div className="record-scrim">
      <div
        className="record-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="record-title"
        ref={root}
        onKeyDown={onKeyDown}
      >
        <div className="dialog-head">
          <div>
            <h2 className="dialog-title" id="record-title" tabIndex={-1} ref={title}>
              Record
            </h2>
            <p className="dialog-sub">Your screen and your voice. Add your camera if you like.</p>
          </div>
          <button
            type="button"
            className="icon-btn"
            tabIndex={0}
            aria-label="Close"
            // From the Record press Esc cancels the take and the dialog stays; this cancels it and closes.
            title={phase === 'starting' ? 'Close' : 'Close (Esc)'}
            onClick={close}
          >
            <Icon name="x" />
          </button>
        </div>
        <div className="record-body">
          {phase === 'loading' && <Waiting />}
          {phase === 'unavailable' && <Unavailable />}
          {phase === 'choosing' && <Choosing takeError={takeError} />}
          {phase === 'starting' && (
            <>
              <Preview>
                <CountdownNumber />
              </Preview>
              <Countdown />
            </>
          )}
        </div>
        <Foot phase={phase} />
        <p className="sr-only" role="status">
          {announced}
        </p>
      </div>
    </div>
  );
}

/** Keeps Tab and Shift Tab inside the dialog. */
function trapFocus(e: KeyboardEvent, root: HTMLElement | null) {
  // In the page's order (a selector list may not be).
  const items = root ? [...root.querySelectorAll<HTMLElement>('*')].filter((el) => el.matches(FOCUSABLE)) : [];
  if (items.length === 0) return;
  const first = items[0];
  const last = items[items.length - 1];
  // The title (focused on opening, before the first control) counts as the start too.
  const atStart = document.activeElement === first || document.activeElement?.id === 'record-title';
  if (e.shiftKey && atStart) {
    e.preventDefault();
    last.focus();
  } else if (!e.shiftKey && document.activeElement === last) {
    e.preventDefault();
    first.focus();
  }
}

/** A spinner and "Getting ready": the setup is read, or the take is arming. */
function Waiting() {
  return (
    <p className="record-wait" role="status">
      <span className="spinner" aria-hidden />
      Getting ready
    </p>
  );
}

/** The setup couldn't be read: why, and Try again (which reads it again). */
function Unavailable() {
  const setupError = useRecorder((s) => s.setupError);
  return (
    <div className="record-said">
      <p role="alert">
        <Icon name="info" />
        {setupError}
      </p>
      <Button variant="secondary" onClick={() => void useRecorder.getState().openRecorder()}>
        Try again
      </Button>
    </div>
  );
}

/** The preview box: 16:9 over the full width. Until the live preview, it shows the clip art. */
function Preview({ children }: { children?: ReactNode }) {
  return (
    <div className="record-preview">
      <span className="record-art" aria-hidden>
        <Icon name="film" />
      </span>
      {children}
    </div>
  );
}

/**
 * The choices: the preview (or the Screen Recording step, or what went wrong, in its place), what to record, the
 * microphone and the camera, and what the setup says.
 */
function Choosing({ takeError }: { takeError: string | null }) {
  const setup = useRecorder((s) => s.setup);
  const step = useRecorder((s) => s.setup !== null && screenStep(s.setup, s.picks, s.screenRefused));
  const error = useRecorder((s) => s.error);
  if (setup === null) return null;
  return (
    <>
      <Preview>
        {step ? (
          <ScreenStep setup={setup} />
        ) : (
          error !== null && <Said text={error} retry={error === takeError} pages={setup.privacySettings} />
        )}
      </Preview>
      <WhatToRecord setup={setup} />
      <Devices setup={setup} />
      <Notes setup={setup} />
    </>
  );
}

/**
 * Why a press didn't go, in the preview's place: an info icon and the sentence. A take that didn't start offers Try
 * again (Record once more); refused the microphone or the camera, it offers that privacy page too. A permission
 * button's refusal offers neither: pressing it again is the way.
 */
function Said({ text, retry, pages }: { text: string; retry: boolean; pages: boolean }) {
  const page = retry && pages ? refusedPage(text) : null;
  return (
    <div className="record-step">
      <p role="alert">
        <Icon name="info" />
        {text}
      </p>
      {retry && (
        <div className="record-actions">
          {page !== null && (
            <Button variant="ghost" onClick={() => void useRecorder.getState().openPrivacy(page)}>
              Open privacy settings
            </Button>
          )}
          <Button variant="secondary" onClick={() => void useRecorder.getState().record()}>
            Try again
          </Button>
        </div>
      )}
    </div>
  );
}

/**
 * The microphone and the camera. A refused one says so under its select while it is chosen; a microphone not asked
 * for yet says macOS asks at the Record press; a camera shows why it won't be recorded at the take's quality, if it
 * won't, and its two toggles.
 */
function Devices({ setup }: { setup: RecorderSetup }) {
  const picks = useRecorder((s) => s.picks);
  const pick = useRecorder((s) => s.pick);
  const pages = setup.privacySettings;
  const fit = cameraNote(setup, picks);
  const fitText = fit === null ? '' : codedMessage(fit);
  return (
    <div className="record-devices">
      <DeviceSelect kind="mic" devices={setup.devices.mics} value={picks.mic} onPick={(mic) => pick({ mic })}>
        {deviceDenied(setup, 'mic', picks) && <DeniedNote kind="mic" pages={pages} />}
        {setup.permissions.mic === 'notAsked' && picks.mic !== null && <MicAskedAtRecord />}
      </DeviceSelect>
      <DeviceSelect
        kind="camera"
        devices={setup.devices.cameras}
        value={picks.camera}
        onPick={(camera) => pick({ camera })}
      >
        {deviceDenied(setup, 'camera', picks) && <DeniedNote kind="camera" pages={pages} />}
        {fitText !== '' && (
          <p className="record-note">
            <Icon name="info" />
            <span>{fitText}</span>
          </p>
        )}
        {picks.camera !== null && (
          <>
            <ToggleRow
              id="record-show-camera"
              label="Show me on screen while recording"
              checked={picks.showCamera}
              onChange={(showCamera) => pick({ showCamera })}
            />
            <ToggleRow
              id="record-mirror"
              label="Mirror my camera"
              checked={picks.mirror}
              onChange={(mirror) => pick({ mirror })}
            />
          </>
        )}
      </DeviceSelect>
    </div>
  );
}

function ToggleRow({
  id,
  label,
  checked,
  onChange,
}: {
  id: string;
  label: string;
  checked: boolean;
  onChange: (checked: boolean) => void;
}) {
  return (
    <div className="record-toggle">
      <Toggle id={id} checked={checked} onChange={onChange} />
      <label htmlFor={id}>{label}</label>
    </div>
  );
}

/** What the setup says before Record (a busy recorder, low space, a device's trouble), in Klipolo's words. */
function Notes({ setup }: { setup: RecorderSetup }) {
  const picks = useRecorder((s) => s.picks);
  const screenRefused = useRecorder((s) => s.screenRefused);
  const waiting = notReady(setup, picks, screenRefused);
  const lines = [
    ...(waiting?.why === 'busy' ? [waiting.text] : []),
    ...notesBefore(setup, picks).map((n) => codedMessage(n)),
  ].filter((text) => text !== '');
  if (lines.length === 0) return null;
  return (
    <ul className="record-notes">
      {lines.map((text, i) => (
        <li key={i} className="record-note">
          <Icon name="info" />
          <span>{text}</span>
        </li>
      ))}
    </ul>
  );
}

/** The footer: where takes go and how to stop one, then Cancel and Record (or the take's Cancel). */
function Foot({ phase }: { phase: Exclude<RecorderPhase, 'recording' | 'saving'> }) {
  const close = useRecorder((s) => s.close);
  const asking = useRecorder((s) => (s.gate?.length ?? 0) > 0);
  return (
    <div className="dialog-foot">
      {(phase === 'choosing' || phase === 'starting') && <span className="left">{STATUS}</span>}
      {phase === 'starting' ? (
        // The take's Cancel: its own element, so the keyboard never stays on it once the choices are back.
        <Button
          key="take"
          variant={asking ? 'ghost' : 'secondary'}
          data-take-cancel
          onClick={() => void useRecorder.getState().cancel()}
        >
          Cancel
        </Button>
      ) : (
        <Button key="dialog" variant="secondary" onClick={close}>
          Cancel
        </Button>
      )}
      {phase === 'choosing' && <RecordButton />}
    </div>
  );
}

/**
 * Record: the only control that starts a take. Off while it can't (a take runs, the setup blocks it, Screen Recording
 * isn't allowed yet, nothing is chosen), still focusable so its tooltip says why.
 */
function RecordButton() {
  const setup = useRecorder((s) => s.setup);
  const picks = useRecorder((s) => s.picks);
  const screenRefused = useRecorder((s) => s.screenRefused);
  if (setup === null) return null;
  const waiting = notReady(setup, picks, screenRefused);
  const button = (
    <Button
      variant="primary"
      aria-disabled={waiting !== null || undefined}
      onClick={() => {
        // While the Screen Recording step shows, a press goes to its button: macOS's own prompt is never the only
        // explanation.
        if (waiting?.why === 'screen') document.getElementById(STEP_BUTTON_ID)?.focus();
        else if (waiting === null) void useRecorder.getState().record();
      }}
    >
      <RecordIcon name="record" />
      Record
    </Button>
  );
  return waiting === null ? (
    button
  ) : (
    <Tooltip label={waiting.text} side="top" describes>
      {button}
    </Tooltip>
  );
}
