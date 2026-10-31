import type { MessageDescriptor } from '@lingui/core';
import { msg } from '@lingui/core/macro';
import type { CSSProperties } from 'react';
import type { BuiltinCaptionStyle } from '../bindings/BuiltinCaptionStyle';
import type { CaptionSettings } from '../bindings/CaptionSettings';
import type { CaptionStyleDef } from '../bindings/CaptionStyleDef';
import type { FormatKind } from '../bindings/FormatKind';
import data from './caption-styles.json';

/**
 * The built-in caption styles as the engine draws them: Rust writes `caption-styles.json` from the style files, so the
 * panel's cards and the engine read the same data. A style is looked up by its id; one this build doesn't have draws in
 * Clean, as the engine does.
 */
const STYLES = data as unknown as CaptionStyleDef[];
const BY_ID = new Map(STYLES.map((s) => [s.id, s]));

/**
 * The styles the Captions panel shows, in its order (after "Off"). Each is an existing builtin; Loud and
 * Arcade stay hidden (a project that has one still draws it).
 */
export const STYLE_ORDER: readonly BuiltinCaptionStyle[] = ['clean', 'bold', 'karaoke', 'highlight', 'boxed'];

export const styleId = (key: BuiltinCaptionStyle) => `klipolo.builtin/caption.${key}`;

/** What Clean draws when a project's style isn't installed (`FALLBACK_STYLE`). */
const FALLBACK = styleId('clean');

export function styleById(id: string | null | undefined): CaptionStyleDef | null {
  return (id && BY_ID.get(id)) || null;
}

/** The style a first enable writes (the model's `default_style`): Bold with a 9:16 or square format, else Clean. */
export function defaultStyle(formats: readonly FormatKind[]): string {
  return formats.some((f) => f === 'portrait' || f === 'square') ? styleId('bold') : FALLBACK;
}

/** The style the project's captions are in (or would be once turned on); one not installed reads as Clean. */
export function styleOf(settings: CaptionSettings, formats: readonly FormatKind[]): CaptionStyleDef {
  return styleById(settings.style ?? defaultStyle(formats)) ?? BY_ID.get(FALLBACK)!;
}

/** Each shown style's name on its card (the design's). */
export const STYLE_NAMES: Partial<Record<BuiltinCaptionStyle, MessageDescriptor>> = {
  clean: msg({ id: 'captions.style.clean', message: 'Clean' }),
  bold: msg({ id: 'captions.style.bold', message: 'Bold pop' }),
  karaoke: msg({ id: 'captions.style.karaoke', message: 'Karaoke' }),
  highlight: msg({ id: 'captions.style.highlight', message: 'Highlight' }),
  boxed: msg({ id: 'captions.style.boxed', message: 'Boxed' }),
};

/**
 * Each shown style's tooltip, in the words chat's `setCaptions` uses: Karaoke and Highlight light up each word as it is
 * said, Bold only pops words in.
 */
export const STYLE_TIPS: Partial<Record<BuiltinCaptionStyle, MessageDescriptor>> = {
  clean: msg({ id: 'captions.style.clean.tip', message: 'Calm subtitles, a sentence at a time' }),
  // As chat's tool says it (`klipolo-chat`'s styles): up to three words a page, each popping in as it is said.
  bold: msg({ id: 'captions.style.bold.tip', message: 'Up to three big words that pop in one by one. No word lights up.' }),
  karaoke: msg({ id: 'captions.style.karaoke.tip', message: 'A line where each word lights up as it is said' }),
  highlight: msg({
    id: 'captions.style.highlight.tip',
    message: 'Big words, with a colored box that lights up each word as it is said',
  }),
  boxed: msg({ id: 'captions.style.boxed.tip', message: 'Sentences on rounded boxes' }),
};

/** A style's key (`bold`); null for a style that isn't a builtin. */
export function styleKey(style: CaptionStyleDef): BuiltinCaptionStyle | null {
  const key = style.id.replace('klipolo.builtin/caption.', '');
  return key !== style.id ? (key as BuiltinCaptionStyle) : null;
}

/** `style` with its look `variant` over it, then `accent` (the person's color): Rust's `CaptionStyleDef::with`. */
export function withLook(
  style: CaptionStyleDef,
  variant: string | null | undefined,
  accent: string | null | undefined,
): CaptionStyleDef {
  const v = variant ? style.variants.find((x) => x.key === variant) : undefined;
  const s: CaptionStyleDef = v
    ? {
        ...style,
        accent: v.accent ?? style.accent,
        fill: v.fill ?? style.fill,
        outline: v.outline ?? style.outline,
        boxes: v.boxes ?? style.boxes,
        shadow: v.shadow ?? style.shadow,
      }
    : style;
  return accent && /^#[0-9a-f]{6}$/i.test(accent) ? { ...s, accent } : s;
}

/** WCAG relative luminance of a `#RRGGBB` color, 0 (black) to 1 (white). */
function luminance(hex: string): number {
  const [r, g, b] = [1, 3, 5].map((at) => {
    const c = parseInt(hex.slice(at, at + 2), 16) / 255;
    return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

/** Black or white, whichever reads on `bg` (Rust's `contrast`: relative luminance above 0.4 is light). */
const contrast = (bg: string) => (luminance(bg) > 0.4 ? '#000000' : '#FFFFFF');

/** The color `paint` draws in (`accent`, `contrast` against the style's box, or a hex), as Rust's `rgb`. */
export function paintOf(style: CaptionStyleDef, paint: string | null, onBox = false): string {
  if (paint === 'accent') return style.accent;
  if (paint === 'contrast' || paint === null) {
    const box = onBox && style.boxes ? paintOf(style, style.boxes.color) : '#000000';
    return contrast(box);
  }
  return paint;
}

const rgba = (hex: string, alpha: number) =>
  `rgba(${parseInt(hex.slice(1, 3), 16)}, ${parseInt(hex.slice(3, 5), 16)}, ${parseInt(hex.slice(5, 7), 16)}, ${alpha})`;

/** A card's sample: the line's CSS, and the spoken word's (its active color, box or glow, or the keyword color). */
export interface SampleCss {
  line: CSSProperties;
  said: CSSProperties;
}

/**
 * How a card draws its sample in `style` (with its look and the person's color): the fill, the outline as a stroke
 * under the fill (`paint-order`), the shadow, a line box with its radius, and the last word as the word being said.
 * Sizes are in em of the card's text.
 */
export function sampleCss(
  base: CaptionStyleDef,
  variant: string | null | undefined,
  accent: string | null | undefined,
): SampleCss {
  const style = withLook(base, variant, accent);
  const lineBox = style.boxes?.on === 'line' ? style.boxes : null;
  const shadows: string[] = [];
  if (style.shadow) {
    const sh = style.shadow;
    shadows.push(`${sh.dxEm}em ${sh.dyEm}em ${sh.blurEm}em ${rgba(paintOf(style, sh.color), sh.alpha)}`);
  }
  const line: CSSProperties = {
    color: lineBox ? paintOf(style, lineBox.text, true) : paintOf(style, style.fill),
    fontFamily: `"${style.font.family}", "Archivo", sans-serif`,
    fontWeight: style.font.weight,
    fontStyle: style.font.italic ? 'italic' : undefined,
    letterSpacing: `${style.letterSpacingEm}em`,
    lineHeight: style.lineHeight,
    textTransform: style.case === 'upper' ? 'uppercase' : 'none',
    textShadow: shadows.length ? shadows.join(', ') : undefined,
  };
  if (style.outline) {
    line.WebkitTextStroke = `${style.outline.widthEm * 2}em ${paintOf(style, style.outline.color)}`;
    line.paintOrder = 'stroke fill';
  }
  if (lineBox) {
    line.backgroundColor = rgba(paintOf(style, lineBox.color), lineBox.alpha);
    line.padding = `${lineBox.padYEm}em ${lineBox.padXEm}em`;
    line.borderRadius = `${lineBox.radiusEm}em`;
    line.boxDecorationBreak = 'clone';
    line.WebkitBoxDecorationBreak = 'clone';
  }
  const said: CSSProperties = {};
  const active = style.active;
  if (active.color) said.color = paintOf(style, active.color.color);
  else if (style.keyword) said.color = paintOf(style, style.keyword.color);
  const glow = active.glow ?? style.keyword?.glow ?? null;
  if (glow) {
    const color = rgba(paintOf(style, glow.color ?? 'accent'), glow.alpha);
    said.textShadow = [`0 0 ${glow.blurEm}em ${color}`, ...shadows].join(', ');
  }
  if (style.boxes?.on === 'activeWord') {
    const b = style.boxes;
    said.backgroundColor = rgba(paintOf(style, b.color), b.alpha);
    said.color = paintOf(style, b.text, true);
    said.padding = `${b.padYEm}em ${b.padXEm}em`;
    said.borderRadius = `${b.radiusEm}em`;
  }
  return { line, said };
}
