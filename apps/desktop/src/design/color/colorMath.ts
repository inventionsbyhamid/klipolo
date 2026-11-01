/** Colour arithmetic for the colour field: colours are `#RRGGBB`, or `#RRGGBBAA` with an alpha. */

const SHORT = /^[0-9a-f]{3}$/i;
const LONG = /^[0-9a-f]{6}$/i;
const WITH_ALPHA = /^[0-9a-f]{8}$/i;

/**
 * A typed or pasted colour as the project stores it, uppercase `#RRGGBB` (`#RRGGBBAA` with `alpha`), or null when it
 * isn't one. Takes 3 or 6 hex digits (8 with `alpha`), with or without a `#`, spaces around them ignored: `ff0` is
 * `#FFFF00`.
 */
export function parseHex(input: string, alpha: boolean): string | null {
  const digits = input.trim().replace(/^#/, '');
  if (SHORT.test(digits)) {
    return `#${[...digits].map((d) => d + d).join('')}`.toUpperCase();
  }
  if (LONG.test(digits) || (alpha && WITH_ALPHA.test(digits))) return `#${digits}`.toUpperCase();
  return null;
}

/** `#RRGGBBAA`, uppercase: no alpha digits is opaque. */
const opaque = (hex: string) => (hex.length === 7 ? withAlpha(hex, 100) : hex.toUpperCase());

/**
 * The same colour, whatever the case of its digits; no alpha digits is opaque, so `#FF0000` is `#FF0000FF` (an alpha
 * setting saved without them: the picker's same colour is no change). Null (none, or the style's own) equals
 * only null.
 */
export function sameColor(a: string | null, b: string | null): boolean {
  if (a === null || b === null) return a === b;
  return opaque(a) === opaque(b);
}

/** The same red, green and blue, whatever the case of the digits and whatever the alpha (`#FFD60A`, `#ffd60a80`). */
export function sameRgb(a: string, b: string): boolean {
  return a.slice(0, 7).toUpperCase() === b.slice(0, 7).toUpperCase();
}

/** A colour's red, green and blue, 0–255 (an alpha is left out). */
export function rgbOf(hex: string): [number, number, number] {
  const channel = (at: number) => parseInt(hex.slice(at, at + 2), 16);
  return [channel(1), channel(3), channel(5)];
}

/**
 * A colour's relative luminance (WCAG 2.x), 0 (black) to 1 (white); an alpha is left out. The one copy of the WCAG
 * maths: the contrast note and the title caret (`textLook`) use it.
 */
export function luminance(hex: string): number {
  const [r, g, b] = rgbOf(hex).map((c) => {
    const s = c / 255;
    return s <= 0.04045 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

/** Under this contrast (3:1, WCAG's least for large text) a colour is hard to read on what surrounds it. */
export const LOW_CONTRAST = 3;

/** The WCAG 2.x contrast ratio of two colours: 1 (the same) to 21 (black and white). Alphas are left out. */
export function contrastRatio(a: string, b: string): number {
  const [la, lb] = [luminance(a), luminance(b)];
  return (Math.max(la, lb) + 0.05) / (Math.min(la, lb) + 0.05);
}

/** A colour as hue (0–360°), saturation and brightness (value), each 0–100; floats, so a drag keeps its precision. */
export interface Hsv {
  h: number;
  s: number;
  v: number;
}

const clamp = (n: number, min: number, max: number) => Math.min(max, Math.max(min, n));
const byte = (n: number) =>
  Math.round(clamp(n, 0, 255))
    .toString(16)
    .padStart(2, '0')
    .toUpperCase();

/** A colour's hue, saturation and brightness (an alpha is left out). A grey has hue 0; black saturation 0 too. */
export function hexToHsv(hex: string): Hsv {
  const [r, g, b] = rgbOf(hex).map((c) => c / 255);
  const max = Math.max(r, g, b);
  const d = max - Math.min(r, g, b);
  let h = 0;
  if (d > 0) {
    if (max === r) h = ((g - b) / d) % 6;
    else if (max === g) h = (b - r) / d + 2;
    else h = (r - g) / d + 4;
    h = h < 0 ? h * 60 + 360 : h * 60;
  }
  return { h, s: max === 0 ? 0 : (d / max) * 100, v: max * 100 };
}

/** The `#RRGGBB` colour of a hue, saturation and brightness (each clamped to its range; the hue wraps). */
export function hsvToHex({ h, s, v }: Hsv): string {
  const hue = ((h % 360) + 360) % 360;
  const sat = clamp(s, 0, 100) / 100;
  const val = clamp(v, 0, 100) / 100;
  const channel = (n: number) => {
    const k = (n + hue / 60) % 6;
    return (val - val * sat * clamp(Math.min(k, 4 - k), 0, 1)) * 255;
  };
  return `#${byte(channel(5))}${byte(channel(3))}${byte(channel(1))}`;
}

/** A colour's opacity, 0–100 % rounded (100 without alpha digits). */
export function alphaOf(hex: string): number {
  return hex.length === 9 ? Math.round((parseInt(hex.slice(7), 16) * 100) / 255) : 100;
}

/** The colour (its red, green and blue) at `percent` opacity, as `#RRGGBBAA`. */
export function withAlpha(hex: string, percent: number): string {
  return `${hex.slice(0, 7).toUpperCase()}${byte((clamp(percent, 0, 100) * 255) / 100)}`;
}

/**
 * A pick for a setting with an alpha: a colour with its own alpha digits (`#RRGGBBAA`: the picker, an 8-digit hex)
 * keeps them; a `#RRGGBB` one (a swatch, a 6-digit hex) takes `base`'s alpha, and stays `#RRGGBB` when `base` has none.
 * So only the Opacity slider and an 8-digit hex change a setting's opacity.
 */
export function keepAlpha(color: string, base: string | null): string {
  if (color.length === 9 || base?.length !== 9) return color;
  return `${color.slice(0, 7)}${base.slice(7).toUpperCase()}`;
}
