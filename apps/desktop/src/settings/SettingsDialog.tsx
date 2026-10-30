import { type CSSProperties, type FormEvent, type KeyboardEvent, useEffect, useRef, useState } from 'react';
import type { AiStatus } from '../bindings/AiStatus';
import { closeSettings, openSettings, refreshAiStatus, useAi } from '../ai/aiStatus';
import { AiIcon } from '../ai/icons';
import { Icon } from '../layout/Icon';
import {
  type BudgetView,
  type PayChoice,
  budgetView,
  initialChoice,
  removePayment,
  saveCode,
  saveKey,
  usd,
} from './settings';
import './settings.css';

const FOCUSABLE = 'button:not(:disabled), input:not(:disabled), [tabindex="0"]';

/**
 * Settings: how Klipolo pays for Meta's models, an access code or the person's own key,
 * Mounted once; opens from the top bar, NeedsKey or Command comma. Esc and the close button close it, and focus goes
 * back where it was. The key is sent to Rust and never shown again: only "Saved" and its last 4 characters.
 */
export function SettingsDialog() {
  const open = useAi((s) => s.settingsOpen);
  useEffect(() => {
    const onKey = (e: globalThis.KeyboardEvent) => {
      if (e.metaKey && !e.ctrlKey && !e.altKey && e.key === ',') {
        e.preventDefault();
        openSettings();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);
  return open ? <Settings /> : null;
}

function Settings() {
  const status = useAi((s) => s.status);
  const [choice, setChoice] = useState<PayChoice>(() => initialChoice(status));
  const dialog = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const trigger = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const first = dialog.current?.querySelector<HTMLElement>(
      'input:not([readonly]), [role="radio"][aria-checked="true"]',
    );
    first?.focus();
    // The credit may have moved since the status was last read.
    void refreshAiStatus();
    return () => trigger?.focus();
  }, []);

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.key === 'Escape') {
      e.preventDefault();
      closeSettings();
    } else if (e.key === 'Tab') {
      trapFocus(e, dialog.current);
    }
  };

  return (
    <div className="scrim settings-scrim">
      <div
        className="dialog settings-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="settings-title"
        ref={dialog}
        onKeyDown={onKeyDown}
      >
        <div className="dialog-head">
          <div>
            <h2 className="dialog-title" id="settings-title">
              Settings
            </h2>
            <p className="dialog-sub">
              Chat, transcripts, captions and stickers use Meta's models. Choose how to pay for them.
            </p>
          </div>
          <button type="button" className="icon-btn" aria-label="Close" title="Close (Esc)" onClick={closeSettings}>
            <AiIcon name="x" />
          </button>
        </div>
        <PayChoices choice={choice} onChoose={setChoice} />
        {choice === 'code' ? <CodeSection status={status} /> : <KeySection status={status} />}
        <div className="dialog-foot">
          <span className="left">Audio and low-resolution video go to Meta's model API.</span>
          <button type="button" className="btn btn-primary" onClick={closeSettings}>
            Done
          </button>
        </div>
      </div>
    </div>
  );
}

const CHOICES: { value: PayChoice; title: string; text: string }[] = [
  { value: 'code', title: 'Access code', text: 'A code with a set budget. No Meta account needed.' },
  { value: 'key', title: 'My Meta API key', text: "Meta bills your key. Kept in your Mac's Keychain." },
];

/** The two radio cards; the arrow keys move between them, as a radio group does. */
function PayChoices({ choice, onChoose }: { choice: PayChoice; onChoose: (choice: PayChoice) => void }) {
  const group = useRef<HTMLDivElement>(null);
  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (!['ArrowLeft', 'ArrowRight', 'ArrowUp', 'ArrowDown'].includes(e.key)) return;
    e.preventDefault();
    const next = choice === 'code' ? 'key' : 'code';
    onChoose(next);
    group.current?.querySelector<HTMLElement>(`[data-choice="${next}"]`)?.focus();
  };
  return (
    <div className="mode-cards" role="radiogroup" aria-label="How to use Meta's models" ref={group} onKeyDown={onKeyDown}>
      {CHOICES.map(({ value, title, text }) => (
        <button
          key={value}
          type="button"
          className="mode"
          role="radio"
          data-choice={value}
          aria-checked={choice === value}
          tabIndex={choice === value ? 0 : -1}
          onClick={() => onChoose(value)}
        >
          <span className="radio" />
          <b>{title}</b>
          <span>{text}</span>
        </button>
      ))}
    </div>
  );
}

const FINE_PRINT =
  'When the code runs out, the AI features pause. Editing and export keep working, and you can switch to your own key at any time.';

function CodeSection({ status }: { status: AiStatus | null }) {
  const saved = status?.mode === 'relay' ? status : null;
  const [code, setCode] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    setBusy(true);
    const failed = await saveCode(code);
    setBusy(false);
    setError(failed);
    if (!failed) setCode('');
  };
  const remove = async () => {
    setBusy(true);
    setError(await removePayment());
    setBusy(false);
  };

  const budget = saved ? budgetView(saved) : null;
  return (
    <>
      <div className="field-v">
        <label htmlFor="settings-code">Access code</label>
        {saved ? (
          <div className="input-row">
            <input id="settings-code" className="input" value={saved.code ?? ''} readOnly />
            <button type="button" className="btn btn-secondary" disabled={busy} onClick={() => void remove()}>
              Remove
            </button>
          </div>
        ) : (
          <form className="input-row" onSubmit={(e) => void submit(e)}>
            <input
              id="settings-code"
              className="input"
              value={code}
              onChange={(e) => setCode(e.target.value)}
              placeholder="KLIP-XXXX-XXXX"
              autoComplete="off"
              autoCapitalize="characters"
              spellCheck={false}
              aria-invalid={error ? true : undefined}
              aria-describedby={error ? 'settings-code-error' : undefined}
            />
            <SubmitButton busy={busy} disabled={!code.trim()} label="Use code" />
          </form>
        )}
        {saved?.active && (
          <span className="status-ok">
            <Icon name="check" />
            Active on this Mac
          </span>
        )}
        {saved?.notice && <Note text={saved.notice} />}
        {error && <FieldError id="settings-code-error" text={error} />}
        {!saved && !error && status?.mode === 'key' && (
          <span className="hint">Using a code replaces your saved key.</span>
        )}
      </div>
      {budget && <Budget view={budget} />}
      <p className="fine">{FINE_PRINT}</p>
    </>
  );
}

function KeySection({ status }: { status: AiStatus | null }) {
  const saved = status?.mode === 'key' ? status : null;
  const [key, setKey] = useState('');
  const [shown, setShown] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    setBusy(true);
    const failed = await saveKey(key);
    setBusy(false);
    setError(failed);
    // A saved key leaves the page at once; a refused one stays so it can be fixed.
    if (!failed) setKey('');
  };
  const remove = async () => {
    setBusy(true);
    setError(await removePayment());
    setBusy(false);
  };

  return (
    <div className="field-v">
      <label htmlFor="settings-key">Meta API key</label>
      {saved ? (
        <div className="input-row">
          <div id="settings-key" className="input is-static">
            {saved.keyHint ? `Saved, ending in ${saved.keyHint}` : 'Saved'}
          </div>
          <button type="button" className="btn btn-secondary" disabled={busy} onClick={() => void remove()}>
            Remove
          </button>
        </div>
      ) : (
        <form className="input-row" onSubmit={(e) => void submit(e)}>
          <span className="secret-field">
            <input
              id="settings-key"
              className="input"
              type={shown ? 'text' : 'password'}
              value={key}
              onChange={(e) => setKey(e.target.value)}
              placeholder="Paste your key"
              autoComplete="off"
              spellCheck={false}
              aria-invalid={error ? true : undefined}
              aria-describedby={error ? 'settings-key-error' : undefined}
            />
            <button
              type="button"
              className="btn btn-ghost btn-sm show-secret"
              aria-pressed={shown}
              onClick={() => setShown((v) => !v)}
            >
              {shown ? 'Hide' : 'Show'}
            </button>
          </span>
          <SubmitButton busy={busy} disabled={!key.trim()} label="Save key" />
        </form>
      )}
      {error ? (
        <FieldError id="settings-key-error" text={error} />
      ) : saved ? (
        <span className="status-ok">
          <Icon name="check" />
          Kept in your Mac's Keychain.
        </span>
      ) : (
        <span className="hint">
          {status?.mode === 'relay'
            ? 'Saving a key replaces your access code. Klipolo checks it with Meta first.'
            : "Klipolo checks it with Meta first, then keeps it in your Mac's Keychain. Get a key at dev.meta.ai."}
        </span>
      )}
    </div>
  );
}

/** Save key and Use code: they check first, so they say "Checking" with a spinner meanwhile. */
function SubmitButton({ busy, disabled, label }: { busy: boolean; disabled: boolean; label: string }) {
  return (
    <button type="submit" className="btn btn-secondary" disabled={busy || disabled}>
      {busy && <span className="spinner" aria-hidden />}
      {busy ? 'Checking' : label}
    </button>
  );
}

function Budget({ view }: { view: BudgetView }) {
  const left = usd(view.left);
  const budget = usd(view.budget);
  return (
    <div className="budget">
      <p className="budget-num">
        {left}
        <small>left of {budget}</small>
      </p>
      <div
        className="progress is-big"
        role="meter"
        aria-label="Credit left"
        aria-valuemin={0}
        aria-valuemax={view.budget}
        aria-valuenow={view.left}
        aria-valuetext={`${left} left of ${budget}`}
        style={{ '--p': `${view.shareLeft * 100}%` } as CSSProperties}
      >
        <i />
      </div>
      <dl>
        <dt>Spent so far</dt>
        <dd>{usd(view.spent)}</dd>
      </dl>
    </div>
  );
}

function FieldError({ id, text }: { id: string; text: string }) {
  return (
    <p className="field-error" id={id} role="alert">
      <Icon name="info" />
      {text}
    </p>
  );
}

function Note({ text }: { text: string }) {
  return (
    <p className="field-note" role="status">
      <Icon name="info" />
      {text}
    </p>
  );
}

/** Keeps Tab and Shift Tab inside the dialog. */
function trapFocus(e: KeyboardEvent, root: HTMLElement | null) {
  const items = root ? [...root.querySelectorAll<HTMLElement>(FOCUSABLE)] : [];
  if (items.length === 0) return;
  const first = items[0];
  const last = items[items.length - 1];
  if (e.shiftKey && document.activeElement === first) {
    e.preventDefault();
    last.focus();
  } else if (!e.shiftKey && document.activeElement === last) {
    e.preventDefault();
    first.focus();
  }
}
