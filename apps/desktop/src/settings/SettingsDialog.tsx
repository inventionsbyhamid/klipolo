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
  sessionView,
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
  // A card picked with the pointer (or Space) puts the cursor in its field; the arrow keys stay on the cards.
  const toField = useRef(false);
  const pick = (next: PayChoice) => {
    toField.current = true;
    setChoice(next);
  };
  useEffect(() => {
    const root = dialog.current;
    const trigger = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    // The empty field, so a code or key can be typed at once; with one saved, the card in use.
    first(root, ['.field-v input:not([readonly])', CHECKED_CARD])?.focus();
    // The credit may have moved since the status was last read.
    void refreshAiStatus();
    // The dialog is modal: no key reaches the editor behind it, and focus that leaves it comes back.
    const onKey = (e: globalThis.KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.preventDefault();
        e.stopPropagation();
        closeSettings();
      } else if (root && !root.contains(e.target as Node)) {
        e.preventDefault();
        e.stopPropagation();
        focusField(root);
      }
    };
    const onFocus = (e: FocusEvent) => {
      if (root && !root.contains(e.target as Node)) focusField(root);
    };
    window.addEventListener('keydown', onKey, true);
    document.addEventListener('focusin', onFocus);
    return () => {
      window.removeEventListener('keydown', onKey, true);
      document.removeEventListener('focusin', onFocus);
      trigger?.focus();
    };
  }, []);
  // Saving replaces the form with the saved row, Remove does the reverse, and picking the other way to pay swaps the
  // field: the focused element can go with them (WebKit doesn't focus a clicked button), so focus moves to the field.
  const mode = status?.mode;
  useEffect(() => {
    const root = dialog.current;
    if (root && (toField.current || !root.contains(document.activeElement))) focusField(root);
    toField.current = false;
  }, [mode, choice]);

  // Keys typed in the dialog stay in it (the editor's shortcuts listen on the window).
  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    e.stopPropagation();
    if (e.key === 'Tab') trapFocus(e, dialog.current);
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
        <PayChoices choice={choice} onPick={pick} onArrow={setChoice} />
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
function PayChoices({
  choice,
  onPick,
  onArrow,
}: {
  choice: PayChoice;
  onPick: (choice: PayChoice) => void;
  onArrow: (choice: PayChoice) => void;
}) {
  const group = useRef<HTMLDivElement>(null);
  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (!['ArrowLeft', 'ArrowRight', 'ArrowUp', 'ArrowDown'].includes(e.key)) return;
    e.preventDefault();
    const next = choice === 'code' ? 'key' : 'code';
    onArrow(next);
    group.current?.querySelector<HTMLElement>(`[data-choice="${next}"]`)?.focus();
  };
  return (
    <div
      className="mode-cards"
      role="radiogroup"
      aria-label="How to use Meta's models"
      ref={group}
      onKeyDown={onKeyDown}
    >
      {CHOICES.map(({ value, title, text }) => (
        <button
          key={value}
          type="button"
          className="mode"
          role="radio"
          data-choice={value}
          aria-checked={choice === value}
          tabIndex={choice === value ? 0 : -1}
          onClick={() => onPick(value)}
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
  const [busy, setBusy] = useState<'save' | 'remove' | null>(null);
  const [error, setError] = useState<string | null>(null);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    setBusy('save');
    const failed = await saveCode(code);
    setBusy(null);
    setError(failed);
    if (!failed) setCode('');
  };
  const remove = async () => {
    setBusy('remove');
    setError(await removePayment());
    setBusy(null);
  };

  const budget = saved ? budgetView(saved) : null;
  return (
    <>
      <div className="field-v">
        <label htmlFor="settings-code">Access code</label>
        {saved ? (
          <div className="input-row">
            <input id="settings-code" className="input" value={saved.code ?? ''} readOnly />
            <button
              type="button"
              className="btn btn-secondary"
              disabled={busy === 'remove'}
              onClick={() => void remove()}
            >
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
            <SubmitButton busy={busy === 'save'} disabled={!code.trim()} label="Use code" />
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
      <SessionSpend status={saved} />
      <p className="fine">{FINE_PRINT}</p>
    </>
  );
}

function KeySection({ status }: { status: AiStatus | null }) {
  const saved = status?.mode === 'key' ? status : null;
  const [key, setKey] = useState('');
  const [shown, setShown] = useState(false);
  const [busy, setBusy] = useState<'save' | 'remove' | null>(null);
  const [error, setError] = useState<string | null>(null);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    setBusy('save');
    const failed = await saveKey(key);
    setBusy(null);
    setError(failed);
    // A saved key leaves the page at once; a refused one stays so it can be fixed.
    if (!failed) setKey('');
  };
  const remove = async () => {
    setBusy('remove');
    setError(await removePayment());
    setBusy(null);
  };

  return (
    <div className="field-v">
      <label htmlFor="settings-key">Meta API key</label>
      {saved ? (
        <div className="input-row">
          {/* Says that a key is saved, never the key: read-only, as the code's field is. */}
          <input
            id="settings-key"
            className="input is-static"
            value={saved.keyHint ? `Saved, ending in ${saved.keyHint}` : 'Saved'}
            readOnly
          />
          <button
            type="button"
            className="btn btn-secondary"
            disabled={busy === 'remove'}
            onClick={() => void remove()}
          >
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
          <SubmitButton busy={busy === 'save'} disabled={!key.trim()} label="Save key" />
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
      <SessionSpend status={saved} />
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

/** What AI cost since Klipolo opened (analysis, chat, pictures): nothing before anything was spent. */
function SessionSpend({ status }: { status: AiStatus | null }) {
  const view = sessionView(status);
  if (!view) return null;
  return (
    <dl className="session-spend" role="group" aria-label="This session">
      <dt>This session</dt>
      <dd>{view.total}</dd>
      <dd className="session-kinds">{view.kinds.join(', ')}</dd>
    </dl>
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

const CHECKED_CARD = '[role="radio"][aria-checked="true"]';

/**
 * The field of the way to pay shown: its input, else (one is saved) the card in use, never Remove, so a second Enter
 * after saving can't take the code or key away again.
 */
function focusField(root: HTMLElement) {
  first(root, ['.field-v input:not([readonly])', CHECKED_CARD])?.focus();
}

/** The first element matching one of `selectors`, tried in that order (not in the page's order). */
function first(root: HTMLElement | null, selectors: string[]): HTMLElement | undefined {
  for (const selector of selectors) {
    const element = root?.querySelector<HTMLElement>(selector);
    if (element) return element;
  }
  return undefined;
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
