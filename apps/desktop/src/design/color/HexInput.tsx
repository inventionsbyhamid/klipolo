import { useId, useRef, useState, type JSX } from 'react';
import { parseHex, sameColor } from './colorMath';

interface HexInputProps {
  label: string;
  /** `#RRGGBB(AA)`, or null (none): an empty field. */
  value: string | null;
  /** The setting takes an alpha: eight digits are a colour too. */
  alpha: boolean;
  /** Why the text isn't a colour, said while it isn't. */
  invalidText: string;
  /** Said when eight digits went into a setting without opacity: the colour is used, its opacity left out. */
  noAlphaText: string;
  /** A valid colour, normalised (`#RRGGBB(AA)`, uppercase): on Enter, or when the field loses focus. */
  onCommit(hex: string): void;
}

const digitsOf = (value: string | null) => (value ? value.replace(/^#/, '').toUpperCase() : '');

/** What the status line says: why the text isn't a colour, or that the opacity of `hex`'s eight digits was left out. */
type Note = { kind: 'invalid' } | { kind: 'noAlpha'; hex: string };

/**
 * A hex colour field, a small input: a `#`, then the digits, uppercase. Enter or leaving the field
 * commits what was typed or pasted when it is a colour (`ff0` is `#FFFF00`; spaces around it don't count, and nothing
 * is ever cut short as it goes in); anything else is marked invalid and says why. Eight digits in a setting without
 * opacity commit the colour and say its opacity was left out (a note, not an error). Escape commits nothing (the
 * popover closing after it blurs the field). The text follows `value` whenever it changes. Every key but Escape and
 * Tab stays in the field, so the editor's shortcuts never see the typing; Escape closes the popover and Tab moves on in
 * it.
 */
export function HexInput({ label, value, alpha, invalidText, noAlphaText, onCommit }: HexInputProps): JSX.Element {
  const [text, setText] = useState(() => digitsOf(value));
  const [note, setNote] = useState<Note | null>(null);
  const [shown, setShown] = useState(value);
  /** Escape was pressed: the blur that follows (the popover closing) commits nothing. */
  const escaped = useRef(false);
  const statusId = useId();
  if (shown !== value) {
    // The colour changed (a pick, an undo): the field shows it.
    setShown(value);
    setText(digitsOf(value));
    // The colour its own commit made keeps the opacity note; any other change clears what the line says.
    setNote((n) => (n?.kind === 'noAlpha' && sameColor(n.hex, value) ? n : null));
  }

  const commit = () => {
    if (escaped.current) {
      escaped.current = false;
      return;
    }
    // Unchanged: nothing to commit, and nothing to complain about (an empty field for none stays quiet).
    if (text.trim().replace(/^#/, '').toUpperCase() === digitsOf(value)) {
      setNote(null);
      return;
    }
    const parsed = parseHex(text, true);
    if (!parsed) {
      setNote({ kind: 'invalid' });
      return;
    }
    const hex = alpha ? parsed : parsed.slice(0, 7);
    setNote(hex === parsed ? null : { kind: 'noAlpha', hex });
    setText(digitsOf(hex));
    onCommit(hex);
  };
  const invalid = note?.kind === 'invalid';

  return (
    <div className="hex-input">
      <div className="hex-field">
        <span aria-hidden>#</span>
        <input
          className="input input-sm hex-digits"
          aria-label={label}
          aria-invalid={invalid || undefined}
          aria-describedby={statusId}
          value={text}
          spellCheck={false}
          autoComplete="off"
          onChange={(e) => {
            setText(e.target.value);
            setNote(null);
          }}
          onBlur={commit}
          onKeyDown={(e) => {
            if (e.key === 'Escape') {
              escaped.current = true;
              return;
            }
            escaped.current = false;
            if (e.key !== 'Tab') e.stopPropagation();
            if (e.key === 'Enter') {
              e.preventDefault();
              commit();
            }
          }}
        />
      </div>
      <p id={statusId} role="status" className="field-note">
        {invalid ? invalidText : note ? noAlphaText : ''}
      </p>
    </div>
  );
}
