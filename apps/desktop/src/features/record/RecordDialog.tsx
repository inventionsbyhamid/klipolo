import { type KeyboardEvent, type MouseEvent, type ReactNode, useEffect, useRef, useState } from 'react';
import type { Previews } from '../../bindings/Previews';
import type { RecorderSetup } from '../../bindings/RecorderSetup';
import { codedMessage } from '../../bridge/call';
import { Button } from '../../design/Button';
import { Toggle } from '../../design/Toggle';
import { Tooltip } from '../../design/Tooltip';
import { Icon } from '../../layout/Icon';
import type { Picks } from '../../lib/recorder/choices';
import { deviceDenied, screenStep } from '../../lib/recorder/permission';
import { phaseOf, type RecorderPhase } from '../../lib/recorder/phase';
import { useRecorder } from '../../state/recorder';
import {
  allowPreview,
  type AskedDevice,
  pausePreviews,
  previewsFor,
  startPreviews,
  stopPreviews,
  useRecorderPreviews,
} from '../../state/recorderPreviews';
import { CameraBubble } from './CameraBubble';
import { DeviceSelect } from './DeviceSelect';
import { RecordIcon } from './icons';
import { Countdown, CountdownNumber, startsIn } from './Countdown';
import { LivePreview } from './LivePreview';
import { MicMeter } from './MicMeter';
import { cameraNote, notesBefore, notReady, refusedPage } from './notes';
import { DeniedNote, ScreenStep, STEP_BUTTON_ID } from './PermissionStep';
import { CARDS_ID, WhatToRecord } from './WhatToRecord';
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

/** Where Klipolo's page is: `front` (focused and shown), `behind` (shown, another app has the focus), or `hidden`. */
type Place = 'front' | 'behind' | 'hidden';

const placeOf = (focused: boolean): Place =>
  document.visibilityState === 'hidden' ? 'hidden' : focused ? 'front' : 'behind';

/** Where Klipolo's page is, as the window says when the dialog opens, then as it changes. */
function usePlace(): Place {
  const [place, setPlace] = useState(() => placeOf(document.hasFocus()));
  useEffect(() => {
    let focused = document.hasFocus();
    const update = () => setPlace(placeOf(focused));
    const blur = () => {
      focused = false;
      update();
    };
    const focus = () => {
      focused = true;
      update();
    };
    window.addEventListener('blur', blur);
    window.addEventListener('focus', focus);
    document.addEventListener('visibilitychange', update);
    return () => {
      window.removeEventListener('blur', blur);
      window.removeEventListener('focus', focus);
      document.removeEventListener('visibilitychange', update);
    };
  }, []);
  return place;
}

/**
 * The previews run only while the choices show and Klipolo is in front: on opening, on every change of choice, and
 * back in front, as the app starts nothing again by itself. They stop on Record (the store stops them first), when
 * the dialog goes, and when the page is hidden (the display asleep, the screen locked: the window may keep the focus,
 * so the app doesn't see it). Behind, the page only stops showing them: the app stops them itself, keeping one that
 * waits for macOS's answer to an Allow press (its question takes the focus but never hides the page).
 */
function usePreviews(phase: RecorderPhase) {
  const place = usePlace();
  const shotsRefused = useRecorderPreviews((s) => s.screenRefused);
  const want = useRecorder((s) =>
    JSON.stringify(phase === 'choosing' ? previewsFor(s.setup, s.picks, s.screenRefused || shotsRefused) : null),
  );
  useEffect(() => {
    const wanted = JSON.parse(want) as Previews | null;
    if (wanted === null || place === 'hidden') stopPreviews();
    else if (place === 'front') startPreviews(wanted);
    else pausePreviews();
  }, [place, want]);
  useEffect(() => () => stopPreviews(), []);
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
  const was = useRef(phase);
  usePreviews(phase);

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
    if (before === 'starting' && phase === 'choosing') title.current?.focus();
    else if (dialog && !dialog.contains(document.activeElement)) title.current?.focus();
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
          {phase === 'choosing' && <Choosing />}
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

/** The name of what is chosen, as its card says it; '' while nothing is. */
function chosenName(setup: RecorderSetup | null, picks: Picks): string {
  const { displays = [], apps = [] } = setup?.devices ?? {};
  const named =
    picks.kind === 'screen'
      ? displays.find((d) => d.id === picks.display)
      : apps.find((a) => a.appId === picks.appId);
  return named?.name ?? '';
}

/**
 * The preview box: 16:9 over the full width, black behind. The live picture of what is chosen, with the camera's
 * bubble over its bottom right; the clip art until a picture comes. What is said in it (the Screen Recording step, why
 * a press didn't go, the countdown) sits over both.
 */
function Preview({ children }: { children?: ReactNode }) {
  const name = useRecorder((s) => chosenName(s.setup, s.picks));
  return (
    <div className="record-preview">
      <span className="record-art" aria-hidden>
        <Icon name="film" />
      </span>
      <LivePreview name={name} />
      <CameraBubble />
      {children}
    </div>
  );
}

/**
 * The choices: the preview (or the Screen Recording step, or what went wrong, in its place), what to record, the
 * microphone and the camera, and what the setup says.
 */
function Choosing() {
  const setup = useRecorder((s) => s.setup);
  const step = useRecorder((s) => s.setup !== null && screenStep(s.setup, s.picks, s.screenRefused));
  const error = useRecorder((s) => s.error);
  if (setup === null) return null;
  return (
    <>
      <Preview>{step ? <ScreenStep setup={setup} /> : error !== null && <Said text={error} setup={setup} />}</Preview>
      <WhatToRecord setup={setup} />
      <Devices setup={setup} />
      <Notes setup={setup} />
    </>
  );
}

/**
 * Why a press didn't go, in the preview's place: an info icon and the sentence. A take that didn't start offers Try
 * again (Record once more); refused the microphone or the camera, it offers that privacy page too, by the refusal's
 * code. Any other press's refusal (a permission button, Record without it, Cancel) offers neither: pressing it again
 * is the way.
 */
function Said({ text, setup }: { text: string; setup: RecorderSetup }) {
  const retry = useRecorder((s) => s.errorFrom === 'take');
  const code = useRecorder((s) => s.errorCode);
  const page = retry && setup.privacySettings ? refusedPage(code) : null;
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
          <RecordPress variant="secondary">Try again</RecordPress>
        </div>
      )}
    </div>
  );
}

/**
 * The microphone and the camera: under each select what can be done about it (`DeviceState`); under the camera, why
 * it won't be recorded at the take's quality, if it won't, and its two toggles.
 */
function Devices({ setup }: { setup: RecorderSetup }) {
  const picks = useRecorder((s) => s.picks);
  const pick = useRecorder((s) => s.pick);
  const fit = cameraNote(setup, picks);
  const fitText = fit === null ? '' : codedMessage(fit);
  return (
    <div className="record-devices">
      <DeviceSelect kind="mic" devices={setup.devices.mics} value={picks.mic} onPick={(mic) => pick({ mic })}>
        <DeviceState kind="mic" setup={setup} />
      </DeviceSelect>
      <DeviceSelect
        kind="camera"
        devices={setup.devices.cameras}
        value={picks.camera}
        onPick={(camera) => pick({ camera })}
      >
        <DeviceState kind="camera" setup={setup} />
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

const ALLOW: Record<AskedDevice, string> = { camera: 'Allow camera', mic: 'Allow microphone' };

/**
 * Under a chosen microphone or camera: macOS refused it (the sentence and "Open privacy settings"), as the setup read
 * or in the question an Allow press raised; not asked yet, "Allow microphone" or "Allow camera", whose press starts
 * its preview, which is when macOS asks (the mic's then turns into the meter, the camera's into the bubble); the mic's
 * level while it comes. Nothing once a take was refused it: the preview's note says so, with its page.
 */
function DeviceState({ kind, setup }: { kind: AskedDevice; setup: RecorderSetup }) {
  const chosen = useRecorder((s) => s.picks[kind] !== null);
  const denied = useRecorder((s) => deviceDenied(setup, kind, s.picks));
  const takeRefused = useRecorder((s) => s.errorFrom === 'take' && s.errorCode === `capture.permission.${kind}`);
  const asked = useRecorderPreviews((s) => s.asked[kind]);
  const refused = useRecorderPreviews((s) => s.refused[kind]);
  const dbfs = useRecorderPreviews((s) => (kind === 'mic' ? s.meterDbfs : null));
  const stopped = useRecorderPreviews((s) => s.micStopped);
  if (!chosen) return null;
  if (denied || refused) return <DeniedNote kind={kind} pages={setup.privacySettings} />;
  if (takeRefused) return null;
  // Not asked yet, or macOS can't say: the press is when it asks (it never asks for a device it refused).
  if ((setup.permissions[kind] === 'notAsked' || setup.permissions[kind] === 'unknown') && !asked)
    return <AllowDevice kind={kind} />;
  // The meter's place is kept while the mic is allowed, empty until a level comes, so neither its first level nor
  // Klipolo going behind and back moves the dialog.
  if (kind === 'mic' && (setup.permissions.mic === 'granted' || asked || dbfs !== null))
    return <MicMeter dbfs={dbfs} off={stopped && dbfs === null} />;
  return null;
}

function AllowDevice({ kind }: { kind: AskedDevice }) {
  const allow = (e: MouseEvent<HTMLButtonElement>) => {
    // The button goes with the press: the keyboard stays in its row, on the device's select.
    e.currentTarget.closest('.record-field')?.querySelector<HTMLElement>('.select')?.focus();
    allowPreview(kind);
  };
  return (
    <Button variant="secondary" size="sm" onClick={allow}>
      {ALLOW[kind]}
    </Button>
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

/** Every take's countdown, in seconds: the app fixes it (a take's `ArmedTake.countdownS` says it again). */
const COUNTDOWN_S = 3;

/** Record's tooltip while it can record: what a press does. */
const READY = `Recording starts after a ${COUNTDOWN_S} s countdown.`;

/**
 * Record, and a failed take's Try again: the only controls that start a take. Off while Record can't (a take runs, the
 * setup blocks it, Screen Recording isn't allowed yet, nothing is chosen), still focusable so its tooltip says why, and
 * a press puts the keyboard where that can be met: the Screen Recording step's button, or the cards to choose from.
 * Always the same element, so the keyboard stays on it as it turns on and off.
 */
function RecordPress({ variant, children }: { variant: 'primary' | 'secondary'; children: ReactNode }) {
  const setup = useRecorder((s) => s.setup);
  const picks = useRecorder((s) => s.picks);
  const screenRefused = useRecorder((s) => s.screenRefused);
  if (setup === null) return null;
  const waiting = notReady(setup, picks, screenRefused);
  const press = () => {
    // While the Screen Recording step shows, a press goes to its button: macOS's own prompt is never the only
    // explanation.
    if (waiting === null) void useRecorder.getState().record();
    else if (waiting.why === 'screen') document.getElementById(STEP_BUTTON_ID)?.focus();
    else if (waiting.why === 'choose') document.querySelector<HTMLElement>(`#${CARDS_ID} [tabindex="0"]`)?.focus();
  };
  return (
    <Tooltip label={waiting?.text ?? READY} side="top" describes={waiting !== null}>
      <Button variant={variant} aria-disabled={waiting !== null || undefined} onClick={press}>
        {children}
      </Button>
    </Tooltip>
  );
}

function RecordButton() {
  return (
    <RecordPress variant="primary">
      <RecordIcon name="record" />
      Record
    </RecordPress>
  );
}
