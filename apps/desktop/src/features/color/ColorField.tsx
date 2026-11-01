import { useLingui } from '@lingui/react/macro';
import { Ban } from 'lucide-react';
import { useState, type JSX, type KeyboardEvent } from 'react';
import { keepAlpha, sameColor, sameRgb } from '../../design/color/colorMath';
import { cn } from '../../design/cn';
import { Popover, PopoverTrigger } from '../../design/Popover';
import { Icon } from '../../layout/Icon';
import { nearestColorName } from '../../lib/swatches';
import { useKeyedPick } from '../../state/keyedPick';
import { useRecentColors } from '../../state/recentColors';
import { ColorPopover } from './ColorPopover';
import { ContrastNote } from './ContrastNote';
import { useKeySettle } from './useKeySettle';
import './color.css';

/** A named colour offered as one tap. */
export interface Swatch {
  value: string;
  name: string;
}

export interface ColorFieldProps {
  /** Names the row, the popover and the "more colors" button. */
  label: string;
  /** `#RRGGBB(AA)`; null: the none choice. */
  value: string | null;
  /** The colours offered as one tap, in order. */
  presets: Swatch[];
  /** null = none: a ⊘ swatch before the presets. */
  none?: { label: string };
  /**
   * The setting takes an alpha: the popover's hex field takes eight digits. Every other pick keeps the setting's alpha,
   * and a swatch is checked by its red, green and blue alone.
   */
  alpha?: boolean;
  /** The colours drawn around this one (a highlight, an outline): under 3:1 against any of them, a note says so. */
  contrastWith?: readonly string[];
  /** A hovered swatch, or one the arrow keys moved to: dry-run it. */
  onPreview?(color: string | null): void;
  /** The pointer left the swatches, or Escape: put the video back. */
  onPreviewEnd?(): void;
  /** A pick: one change. Picking the colour the setting has commits nothing. */
  onCommit(color: string | null): void;
}

/** One choice of the row: a colour, or none. */
interface Choice {
  value: string | null;
  name: string;
}

/**
 * One colour setting, wherever a colour is picked: a row of 20 px swatches (none first when the
 * setting has it), each one tap, then "+", which opens the color popover. A radio group: the arrow keys move the
 * checked swatch, a run of them one change once the keys stop (`useKeyedPick`). Hovering a swatch, or arrowing onto
 * it, dry-runs it (`onPreview`); leaving the row, Escape or closing the popover puts the video back (`onPreviewEnd`); a
 * pick commits once (`onCommit`), and picking the colour it has commits nothing. In an `alpha` field a `#RRGGBB` pick
 * takes the setting's alpha; a `#RRGGBBAA` one keeps its own. A custom colour shows itself on the "+" button, named by
 * the colour it is nearest. With `contrastWith`, a note says when the colour is hard to read on what surrounds it.
 */
export function ColorField(props: ColorFieldProps): JSX.Element {
  const { label, value, presets, none, alpha = false, contrastWith, onCommit } = props;
  const { t, i18n } = useLingui();
  const onPreview = props.onPreview ?? (() => undefined);
  const onPreviewEnd = props.onPreviewEnd ?? (() => undefined);
  const keep = (c: string | null) => (c === null || !alpha ? c : keepAlpha(c, value));
  const isValue = (c: string | null) => sameColor(c, value);
  const preview = (c: string | null) => {
    const kept = keep(c);
    if (isValue(kept)) onPreviewEnd();
    else onPreview(kept);
  };
  /** Commits `c` (kept); true unless it is the colour the setting has, which commits nothing. */
  const pick = (c: string | null): boolean => {
    const kept = keep(c);
    if (isValue(kept)) {
      onPreviewEnd();
      return false;
    }
    onCommit(kept);
    return true;
  };
  /**
   * A committed custom colour goes to Recent, unless the field offers it already (a preset) or only its opacity changed
   * (the colour is the setting's own: Recent holds colours, so nothing new to put first).
   */
  const remember = (hex: string) => {
    if (value !== null && sameRgb(hex, value)) return;
    if (presets.some((p) => sameRgb(p.value, hex))) return;
    useRecentColors.getState().push(hex);
  };
  /** The custom picker's keys, settled into one pick 400 ms after the last (sooner on closing, undo or unmount). */
  const settle = useKeySettle((hex) => {
    if (pick(hex)) remember(hex);
  });
  /** Bumped when a pick replaced keys still settling: the picker shows the setting's colour again. */
  const [resync, setResync] = useState(0);
  /** A pick replaces keys still settling (one undo step: the pick), so the keyed colour never lands on top of it. */
  const commit = (c: string | null) => {
    if (settle.drop()) setResync((n) => n + 1);
    return pick(c);
  };
  /** A custom colour: one pick, as `commit`; Recent keeps it. */
  const pickCustom = (hex: string) => {
    if (commit(hex)) remember(hex);
  };
  const keys = useKeyedPick<string | null>(value, (c) => void pick(c));
  const choices: Choice[] = [...(none ? [{ value: null, name: none.label }] : []), ...presets];
  /** A choice is the colour shown: with an alpha, by its red, green and blue alone. */
  const matches = (choice: string | null, shown: string | null) =>
    choice === null || shown === null ? choice === shown : alpha ? sameRgb(choice, shown) : sameColor(choice, shown);
  const checked = choices.findIndex((c) => matches(c.value, keys.shown));
  const custom = value !== null && !choices.some((c) => matches(c.value, value));
  const name = value === null ? (none?.label ?? '') : i18n._(nearestColorName(value));

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.key === 'Escape') {
      // The arrow keys' run is dropped before the video is put back: it never lands after.
      keys.cancel();
      onPreviewEnd();
      return;
    }
    const step = e.key === 'ArrowRight' || e.key === 'ArrowDown' ? 1 : e.key === 'ArrowLeft' || e.key === 'ArrowUp' ? -1 : 0;
    if (!step || !(e.target as HTMLElement).dataset.choice) return;
    e.preventDefault();
    const at = (Math.max(0, checked) + step + choices.length) % choices.length;
    const next = choices[at];
    keys.key(next.value);
    preview(next.value);
    e.currentTarget.querySelector<HTMLElement>(`[data-choice="${at}"]`)?.focus();
  };
  const more = custom
    ? t({
        id: 'color.custom.current',
        message: `${label}: ${name}, more colors`,
        comment: '{label} names the setting; {name} the custom color it holds',
      })
    : t({
        id: 'color.custom',
        message: `${label}: more colors`,
        comment: '{label} is e.g. Text color; opens the color popover',
      });

  return (
    <div className="color-field">
      <div
        className="swatches"
        role="radiogroup"
        aria-label={label}
        onKeyDown={onKeyDown}
        onPointerLeave={onPreviewEnd}
        onBlur={keys.onBlur}
      >
        {choices.map((choice, i) => (
          <button
            key={choice.value ?? 'none'}
            type="button"
            role="radio"
            data-choice={i}
            aria-checked={i === checked}
            aria-label={choice.name}
            tabIndex={i === checked || (checked < 0 && i === 0) ? 0 : -1}
            className={cn('swatch', choice.value === null && 'none')}
            // The fill is a content colour the person can pick, so it's data, not a token.
            style={choice.value === null ? undefined : { ['--c' as string]: choice.value }}
            onPointerEnter={() => preview(choice.value)}
            onClick={() => keys.pick(choice.value)}
          >
            {choice.value === null && <Ban size={12} aria-hidden />}
          </button>
        ))}
        <Popover
          onOpenChange={(open) => {
            if (open) return;
            settle.flush();
            onPreviewEnd();
          }}
        >
          <PopoverTrigger asChild>
            <button
              type="button"
              tabIndex={0}
              className={cn('swatch add', custom && 'custom')}
              aria-label={more}
              style={custom ? { ['--c' as string]: value } : undefined}
            >
              {!custom && <Icon name="plus" />}
            </button>
          </PopoverTrigger>
          <ColorPopover
            label={label}
            value={value}
            selected={alpha && value ? value.slice(0, 7) : value}
            resync={resync}
            alpha={alpha}
            presets={presets}
            none={none ? { value: null, name: none.label, kind: 'none' } : null}
            onPreview={preview}
            onPreviewEnd={onPreviewEnd}
            onPick={(c) => void commit(c)}
            onCustom={pickCustom}
            onDrag={preview}
            onRelease={pickCustom}
            onKey={(hex) => {
              preview(hex);
              settle.keyed(keep(hex) ?? hex);
            }}
            onCancel={onPreviewEnd}
          />
        </Popover>
      </div>
      {contrastWith && <ContrastNote color={value} against={contrastWith} />}
    </div>
  );
}
