import { useLingui } from '@lingui/react/macro';
import type { ReactNode } from 'react';
import type { Op } from '../../bindings/Op';
import type { Label } from '../../lib/labels';
import { commitLatest } from '../../state/latest';
import { Field, Swatches } from './fields';
import { textSwatches } from './TextInspector';
import { ToolSlider } from './ToolSlider';

/** The colour without its alpha: `#RRGGBB`, upper case. */
const rgbOf = (color: string) => color.slice(0, 7).toUpperCase();

/** The colour's alpha, 0 to 1 (`#RRGGBBAA`); a `#RRGGBB` colour is opaque. */
export function alphaOf(color: string): number {
  const hex = color.length === 9 ? color.slice(7, 9) : 'FF';
  const value = Number.parseInt(hex, 16);
  return Number.isFinite(value) ? value / 255 : 1;
}

/** `rgb` (a `#RRGGBB` pick) with alpha `alpha` as two hex digits: `#RRGGBBAA`. */
function withAlpha(rgb: string, alpha: number): string {
  const byte = Math.round(Math.min(1, Math.max(0, alpha)) * 255);
  return `${rgbOf(rgb)}${byte.toString(16).padStart(2, '0').toUpperCase()}`;
}

/** `rgb` with the alpha digits `base` holds; as it is when `base` has none (it is opaque). */
const keepAlpha = (rgb: string, base: string) => (base.length === 9 ? `${rgbOf(rgb)}${base.slice(7).toUpperCase()}` : rgbOf(rgb));

/** The same colour, whatever the case of its digits and whether an opaque one spells out its alpha. */
const sameColor = (a: string, b: string) => withAlpha(a, alphaOf(a)) === withAlpha(b, alphaOf(b));

const percent = (v: number) => `${Math.round(v * 100)}%`;

interface ParamColorProps {
  label: string;
  /** `#RRGGBB` or `#RRGGBBAA`. */
  value: string;
  /** The setting takes an alpha (`#RRGGBBAA`): Opacity changes it; every other pick keeps the alpha it has. */
  alpha: boolean;
  opsFor: (color: string) => Op[];
  commitLabel: Label;
  /** The colour as the project has it when a change goes in (a pick keeps the alpha it has then). */
  valueNow?: () => string;
  /** Beside the swatches: the setting's Reset, when it has one. */
  reset?: ReactNode;
}

/**
 * An effect's or generator's colour setting: the card's swatches, with "+" for any colour as hex. A pick is one
 * change, and picking the colour it has commits nothing. A setting with an alpha keeps the alpha it holds when a
 * swatch or a hex is picked, and has Opacity under its swatches for that alone (a slider, one change per gesture,
 * dry-run as it moves).
 */
export function ParamColor({ label, value, alpha, opsFor, commitLabel, valueNow, reset }: ParamColorProps) {
  const { t } = useLingui();
  const latest = () => valueNow?.() ?? value;
  /** The colour a `#RRGGBB` pick sets: with the alpha the setting holds then, when it takes one. */
  const kept = (rgb: string) => (alpha ? keepAlpha(rgb, latest()) : rgbOf(rgb));
  const pick = (rgb: string) => {
    if (sameColor(kept(rgb), latest())) return;
    void commitLatest(commitLabel, () => opsFor(kept(rgb)));
  };
  return (
    <>
      <Field label={label}>
        <div className="param-control">
          <Swatches label={label} value={value} swatches={textSwatches(t)} onPick={pick} />
          {reset}
        </div>
      </Field>
      {alpha && (
        <Field label={t({ id: 'param.opacity', message: 'Opacity', comment: 'How see-through a colour setting is' })}>
          <ToolSlider
            label={t({
              id: 'param.opacity.of',
              message: `${label} opacity`,
              comment: '{label} is a colour setting, e.g. Border color',
            })}
            commitLabel={commitLabel}
            value={Math.round(alphaOf(value) * 100) / 100}
            min={0}
            max={1}
            step={0.01}
            format={percent}
            ops={(a) => opsFor(withAlpha(latest(), a))}
          />
        </Field>
      )}
    </>
  );
}
