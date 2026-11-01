import type { FontRef } from '../../bindings/FontRef';
import type { ParamSpec } from '../../bindings/ParamSpec';
import type { ParamValue } from '../../bindings/ParamValue';
import type { PointN } from '../../bindings/PointN';
import type { TableRow } from '../../bindings/TableRow';
import { activeIntlLocale } from '../../i18n';
import { formatNumber, formatPercent, formatPixels, formatSeconds } from '../../lib/format';

/** A contribution's stored settings: missing keys take the schema's defaults. */
export type ParamValues = Partial<Record<string, ParamValue>>;

export function defaultOf(spec: ParamSpec): ParamValue | null {
  return 'default' in spec ? (spec.default as ParamValue) : null;
}

/** A font setting's value: a `FontRef`, never a family name. */
const isFontRef = (value: unknown): value is FontRef =>
  typeof value === 'object' &&
  value !== null &&
  typeof (value as FontRef).family === 'string' &&
  typeof (value as FontRef).source === 'object';

/** A table's rows. */
const isRows = (value: unknown): value is TableRow[] => Array.isArray(value);

const isPoint = (value: unknown): value is PointN =>
  typeof value === 'object' &&
  value !== null &&
  typeof (value as PointN).x === 'number' &&
  typeof (value as PointN).y === 'number';

/** Whether `value` has the shape `spec` stores (a choice must be one of its options). */
export function fits(spec: ParamSpec, value: ParamValue | undefined): value is ParamValue {
  switch (spec.type) {
    case 'float':
    case 'int':
    case 'angle':
      return typeof value === 'number' && Number.isFinite(value);
    case 'bool':
      return typeof value === 'boolean';
    case 'point':
      return isPoint(value);
    case 'choice':
      return typeof value === 'string' && spec.options.includes(value);
    case 'font':
      return isFontRef(value);
    case 'table':
      return isRows(value);
    default:
      return typeof value === 'string';
  }
}

/** The stored value, or the schema's default (a value of the wrong shape counts as missing). */
export function valueOf(spec: ParamSpec, values: ParamValues): ParamValue | null {
  const stored = values[spec.key];
  return fits(spec, stored) ? stored : defaultOf(spec);
}

/** Whether a value is stored for `spec` (so Reset can take it back to the default). */
export function isStored(spec: ParamSpec, values: ParamValues): boolean {
  return fits(spec, values[spec.key]);
}

/**
 * A value as the card shows it, in the language shown: "12 px", "50%", "90°", "0.5 s". Pixels and degrees are symbols
 * every language uses, like seconds (`formatSeconds`).
 */
export function formatParam(spec: ParamSpec, value: ParamValue | null): string {
  const locale = activeIntlLocale();
  if (isPoint(value)) return `${formatPercent(locale, value.x)}, ${formatPercent(locale, value.y)}`;
  if (typeof value !== 'number') return String(value ?? '');
  if (spec.type === 'angle') return `${formatNumber(locale, Math.round(value))}°`;
  if (spec.type === 'float') {
    if (spec.unit === 'px') {
      // A short range (a feather of 0 to 8 px) shows tenths; a long one (a blur of 0 to 64 px) whole pixels.
      const digits = spec.max - spec.min <= 10 ? 1 : 0;
      return formatPixels(locale, value, digits);
    }
    if (spec.unit === 'percent') return formatPercent(locale, value);
    if (spec.unit === 'seconds') return formatSeconds(locale, value);
  }
  return formatNumber(locale, value, { maximumFractionDigits: 2 });
}

/** Choice options are stored keys ("top-left"); shown in sentence case ("Top left"). */
export function choiceLabel(option: string): string {
  const words = option.replace(/[-_]/g, ' ');
  return words.charAt(0).toUpperCase() + words.slice(1);
}

/**
 * The step a float takes without one in its schema: what `formatParam` shows, so each press changes what is read out:
 * whole pixels over 10 px (a blur), tenths up to 10 px (a feather), whole percents, tenths of a second, else 1/100 of
 * the range.
 */
function floatStep(spec: Extract<ParamSpec, { type: 'float' }>): number {
  if (spec.step !== null) return spec.step;
  if (spec.unit === 'px') return spec.max - spec.min > 10 ? 1 : 0.1;
  if (spec.unit === 'percent') return 0.01;
  if (spec.unit === 'seconds') return 0.1;
  return (spec.max - spec.min) / 100;
}

/** A slider's range and step for a number setting: angles from -360 to 360°, whole steps for int and angle. */
export function sliderRange(spec: ParamSpec): { min: number; max: number; step: number } | null {
  if (spec.type === 'angle') return { min: -360, max: 360, step: 1 };
  if (spec.type === 'int') return { min: spec.min, max: spec.max, step: 1 };
  if (spec.type === 'float') return { min: spec.min, max: spec.max, step: floatStep(spec) };
  return null;
}
