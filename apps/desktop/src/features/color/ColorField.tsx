import { useLingui } from '@lingui/react/macro';
import { Ban, RotateCcw } from 'lucide-react';
import { useState, type JSX, type KeyboardEvent } from 'react';
import { keepAlpha, sameColor, sameRgb } from '../../design/color/colorMath';
import type { SwatchOption } from '../../design/color/SwatchList';
import { cn } from '../../design/cn';
import { Popover, PopoverTrigger } from '../../design/Popover';
import { Icon } from '../../layout/Icon';
import { nearestColorName } from '../../lib/swatches';
import { useKeyedPick } from '../../state/keyedPick';
import { useRecentColors } from '../../state/recentColors';
import { ColorPopover } from './ColorPopover';
import { ContrastNote } from './ContrastNote';
import { numbered } from './numbered';
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
  /** `#RRGGBB(AA)`; null: the none or the style's own choice. */
  value: string | null;
  /** The colours offered as one tap, in order. */
  presets: Swatch[];
  /** A caption style's own colours: inline, they replace the presets (named by the colour each is nearest). */
  styleColors?: string[];
  /** null = the style's own: a reset swatch first. */
  inherit?: { label: string };
  /** null = none: a ⊘ swatch before the presets. */
  none?: { label: string };
  /**
   * The setting takes an alpha: the popover's hex field takes eight digits. Every other pick keeps the setting's alpha,
   * and a swatch is checked by its red, green and blue alone.
   */
  alpha?: boolean;
  /**
   * The colours drawn around this one (a highlight, an outline): under 3:1 against any of them, a note says so, under
   * the row, or at the popover's foot when compact.
   */
  contrastWith?: readonly string[];
  /**
   * The setting holds several colours (caption words with different ones; `value` is then null): no swatch is checked,
   * and every pick commits, the style's own too.
   */
  mixed?: boolean;
  /** One swatch button that opens the popover, instead of the row (a bar of controls). */
  compact?: boolean;
  /** A hovered swatch, or one the arrow keys moved to: dry-run it. */
  onPreview?(color: string | null): void;
  /** The pointer left the swatches, or Escape: put the video back. */
  onPreviewEnd?(): void;
  /** A pick: one change. Picking the colour the setting has commits nothing. */
  onCommit(color: string | null): void;
}

/** One choice of the row: a colour, none, or the style's own. */
interface Choice {
  value: string | null;
  name: string;
  kind?: 'none' | 'inherit';
}

/**
 * One colour setting, wherever a colour is picked: a row of 20 px swatches (the style's own or
 * none first when the setting has it, then the style's colours or the presets), each one tap, then "+", which opens
 * the color popover. Compact, it is one swatch button named "{label}: {colour}" that opens the popover. The row is a
 * radio group: the arrow keys move the checked swatch, a run of them one change once the keys stop (`useKeyedPick`),
 * and Escape drops the run. Hovering a swatch, or arrowing onto it, dry-runs it (`onPreview`); leaving the row, Escape
 * or closing the popover puts the video back (`onPreviewEnd`); a pick commits once (`onCommit`), and picking the colour
 * it has commits nothing (except while `mixed`). In an `alpha` field a `#RRGGBB` pick takes the setting's alpha; a
 * `#RRGGBBAA` one keeps its own. A custom colour shows itself on the "+" button, named by the colour it is nearest.
 * With `contrastWith`, a note says when the colour is hard to read on what surrounds it.
 */
export function ColorField(props: ColorFieldProps): JSX.Element {
  const { label, value, presets, styleColors, inherit, none, alpha = false, contrastWith, onCommit } = props;
  const { mixed = false, compact = false } = props;
  const { t, i18n } = useLingui();
  const onPreview = props.onPreview ?? (() => undefined);
  const onPreviewEnd = props.onPreviewEnd ?? (() => undefined);
  const keep = (c: string | null) => (c === null || !alpha ? c : keepAlpha(c, value));
  /** The colour the setting has (nothing to dry-run or commit); none while it holds several (`mixed`). */
  const isValue = (c: string | null) => !mixed && sameColor(c, value);
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
   * A committed custom colour goes to Recent, unless the field offers it already (a preset, a style's colour) or only
   * its opacity changed (the colour is the setting's own: Recent holds colours, so nothing new to put first).
   */
  const remember = (hex: string) => {
    if (value !== null && sameRgb(hex, value)) return;
    if (presets.some((p) => sameRgb(p.value, hex)) || styleColors?.some((c) => sameRgb(c, hex))) return;
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
  /** The reset swatch: the style's own (it wins) or none. */
  const reset: Choice | null = inherit
    ? { value: null, name: inherit.label, kind: 'inherit' }
    : none
      ? { value: null, name: none.label, kind: 'none' }
      : null;
  const styled: Swatch[] | null = styleColors
    ? styleColors.map((hex) => ({ value: hex.toUpperCase(), name: i18n._(nearestColorName(hex)) }))
    : null;
  const choices: Choice[] = numbered([...(reset ? [reset] : []), ...(styled ?? presets)], (name, n) =>
    t({
      id: 'color.numbered',
      message: `${name} ${n}`,
      comment: '{name} is a color’s name; {n} tells apart two swatches with the same name, e.g. “Yellow 2”',
    }),
  );
  /** A choice is the colour shown: with an alpha, by its red, green and blue alone. */
  const matches = (choice: string | null, shown: string | null) =>
    choice === null || shown === null ? choice === shown : alpha ? sameRgb(choice, shown) : sameColor(choice, shown);
  const checked = mixed ? -1 : choices.findIndex((c) => matches(c.value, keys.shown));
  const custom = !mixed && value !== null && !choices.some((c) => matches(c.value, value));
  const preset = value === null ? undefined : [...(styled ?? []), ...presets].find((p) => matches(p.value, value));
  const name = mixed
    ? t({
        id: 'color.mixed',
        message: 'Mixed',
        comment: 'The color of things selected together that have different colors, e.g. “Word color: Mixed”',
      })
    : value === null
      ? (reset?.name ?? '')
      : (preset?.name ?? i18n._(nearestColorName(value)));

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

  const popover = (
    <ColorPopover
      label={label}
      value={value}
      selected={mixed ? undefined : alpha && value ? value.slice(0, 7) : value}
      resync={resync}
      alpha={alpha}
      presets={presets}
      styled={styled}
      reset={reset ? ({ value: null, name: reset.name, kind: reset.kind } satisfies SwatchOption) : null}
      contrastWith={compact ? contrastWith : undefined}
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
  );
  const onOpenChange = (open: boolean) => {
    if (open) return;
    settle.flush();
    onPreviewEnd();
  };

  if (compact) {
    const fieldName = t({
      id: 'color.field.named',
      message: `${label}: ${name}`,
      comment: '{label} names the setting; {name} its color, e.g. “Word color: Yellow”',
    });
    return (
      <Popover onOpenChange={onOpenChange}>
        <PopoverTrigger asChild>
          <button type="button" className="select color-compact" aria-label={fieldName}>
            <span
              aria-hidden
              className={cn('color-compact-dot', mixed && 'mixed')}
              // The fill is the colour the setting holds, so it's data, not a token.
              style={value && !mixed ? { ['--c' as string]: value } : undefined}
            >
              {value === null && !mixed && (inherit ? <RotateCcw size={10} /> : <Ban size={10} />)}
            </span>
            <span className="select-text">{label}</span>
            <Icon name="chev-down" />
          </button>
        </PopoverTrigger>
        {popover}
      </Popover>
    );
  }

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
            key={`${choice.kind ?? 'color'}:${choice.value ?? ''}`}
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
            {choice.kind === 'none' && <Ban size={12} aria-hidden />}
            {choice.kind === 'inherit' && <RotateCcw size={12} aria-hidden />}
          </button>
        ))}
        <Popover onOpenChange={onOpenChange}>
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
          {popover}
        </Popover>
      </div>
      {contrastWith && <ContrastNote color={value} against={contrastWith} />}
    </div>
  );
}
