import { useId, useState, type KeyboardEvent, type ReactNode } from 'react';
import type { Anchor } from '../../bindings/Anchor';
import { cn } from '../../design/cn';
import { Menu, MenuContent, MenuRadioGroup, MenuRadioItem, MenuTrigger } from '../../design/Menu';
import { Icon } from '../../layout/Icon';
import { useKeyedPick } from '../../state/keyedPick';
import { ColorField, type Swatch } from '../color/ColorField';

/**
 * A row of the inspector: its label on the left (66 px, faint), the control on the right; `stack` puts
 * the label above (Animation, Timing, and Background in a narrow window).
 */
export function Field({
  label,
  children,
  stack = false,
  narrowStack = false,
  labelFor,
}: {
  label: string;
  children: ReactNode;
  stack?: boolean;
  narrowStack?: boolean;
  labelFor?: string;
}) {
  return (
    <div className={cn('field', stack && 'field-col', narrowStack && 'field-narrow-col')}>
      {labelFor ? (
        <label className="field-label" htmlFor={labelFor}>
          {label}
        </label>
      ) : (
        <span className="field-label">{label}</span>
      )}
      {children}
    </div>
  );
}

/** A named colour offered as one tap. */
export type { Swatch } from '../color/ColorField';

/**
 * Colour swatches: 20 px circles, the checked one ringed in paper; the last, "+", opens the color
 * popover for any other colour (`ColorField`, without a dry run). A radio group: the arrow keys move, a run of them one
 * change.
 */
export function Swatches({
  label,
  value,
  swatches,
  onPick,
}: {
  label: string;
  value: string;
  swatches: Swatch[];
  onPick: (color: string) => void;
}) {
  return (
    <ColorField
      label={label}
      value={value}
      presets={swatches}
      // A colour with opacity is checked by its red, green and blue, and a pick keeps the opacity it holds.
      alpha={value.length === 9}
      onCommit={(color) => color !== null && onPick(color)}
    />
  );
}

const CELLS: Anchor[] = [
  'topLeft',
  'topCenter',
  'topRight',
  'middleLeft',
  'center',
  'middleRight',
  'bottomLeft',
  'bottomCenter',
  'bottomRight',
];

/**
 * The 3 x 3 position grid: the checked cell is paper with an on-paper dot. The arrow keys move, a run of them
 * one change (`useKeyedPick`).
 */
export function PositionGrid({
  label,
  value,
  onPick,
  names,
}: {
  label: string;
  value: Anchor;
  onPick: (anchor: Anchor) => void;
  names: Record<Anchor, string>;
}) {
  const keys = useKeyedPick(value, onPick);
  const shown = keys.shown;
  const at = CELLS.indexOf(shown);
  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    const move: Record<string, [number, number]> = {
      ArrowLeft: [-1, 0],
      ArrowRight: [1, 0],
      ArrowUp: [0, -1],
      ArrowDown: [0, 1],
    };
    const d = move[e.key];
    if (!d) return;
    e.preventDefault();
    const col = Math.min(2, Math.max(0, (at % 3) + d[0]));
    const row = Math.min(2, Math.max(0, Math.floor(at / 3) + d[1]));
    const next = CELLS[row * 3 + col];
    if (next !== shown) keys.key(next);
    e.currentTarget.querySelector<HTMLElement>(`[data-cell="${next}"]`)?.focus();
  };
  return (
    <div className="pos-grid" role="radiogroup" aria-label={label} onKeyDown={onKeyDown} onBlur={keys.onBlur}>
      {CELLS.map((cell) => (
        <button
          key={cell}
          type="button"
          role="radio"
          data-cell={cell}
          aria-checked={cell === shown}
          aria-label={names[cell]}
          tabIndex={cell === shown ? 0 : -1}
          className={cell === shown ? 'on' : undefined}
          onClick={() => keys.pick(cell)}
        />
      ))}
    </div>
  );
}

/**
 * A select: a 30 px button with a chevron that opens a menu of choices; `prefix` reads "In: Pop". A choice's
 * `title` is its tooltip (the id of one this version lacks).
 */
export function Select<T extends string>({
  label,
  value,
  choices,
  onPick,
  prefix,
}: {
  label: string;
  value: T;
  choices: { value: T; label: string; title?: string }[];
  onPick: (value: T) => void;
  prefix?: string;
}) {
  const shown = choices.find((c) => c.value === value)?.label ?? value;
  return (
    <Menu>
      <MenuTrigger asChild>
        <button type="button" className="select" aria-label={`${label}: ${shown}`}>
          <span className="select-text">{prefix ? `${prefix}: ${shown}` : shown}</span>
          <Icon name="chev-down" />
        </button>
      </MenuTrigger>
      <MenuContent>
        <MenuRadioGroup label={label} value={value} onValueChange={(v) => onPick(v as T)}>
          {choices.map((c) => (
            <MenuRadioItem key={c.value} value={c.value}>
              {c.title ? <span title={c.title}>{c.label}</span> : c.label}
            </MenuRadioItem>
          ))}
        </MenuRadioGroup>
      </MenuContent>
    </Menu>
  );
}

/**
 * A small text field that commits on Enter or when it is left, and puts back what it had on Escape: a time ("0:03.0")
 * or a length ("3.0 s"). `parse` reads what was typed (null: not a value, put back).
 */
export function ValueField({
  label,
  value,
  parse,
  onCommit,
}: {
  label: string;
  value: string;
  parse: (text: string) => number | null;
  onCommit: (value: number) => void;
}) {
  const id = useId();
  const [draft, setDraft] = useState<string | null>(null);
  const done = (keep: boolean) => {
    if (draft === null) return;
    const read = keep ? parse(draft) : null;
    setDraft(null);
    if (read !== null) onCommit(read);
  };
  return (
    <span className="input input-sm value-field">
      <label className="field-label" htmlFor={id}>
        {label}
      </label>
      <input
        id={id}
        className="value-input"
        value={draft ?? value}
        onFocus={(e) => e.currentTarget.select()}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={() => done(true)}
        onKeyDown={(e) => {
          if (e.key === 'Enter') {
            e.preventDefault();
            done(true);
          } else if (e.key === 'Escape') {
            e.preventDefault();
            e.stopPropagation();
            done(false);
          }
        }}
      />
    </span>
  );
}
