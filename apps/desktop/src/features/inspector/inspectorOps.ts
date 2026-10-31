import type { Anchor } from '../../bindings/Anchor';
import type { BoxN } from '../../bindings/BoxN';
import type { FormatKind } from '../../bindings/FormatKind';
import type { Layout } from '../../bindings/Layout';
import type { Motion } from '../../bindings/Motion';
import type { TextStyle } from '../../bindings/TextStyle';
import { builtinId } from '../../lib/contrib';
import { layoutPatchFor } from '../../lib/geometry';
import { anchorForRect, resolveRect } from '../../lib/layout';
import { safeRect } from '../../lib/safeAreas';

/** How long an animation picked in the inspector runs (0.3 s), at most the layer's length. */
export const ANIMATION_US = 300_000;

/** The ways a layer comes in and goes out. "Slide up" comes in from below; "Slide down" goes out below. */
export type InChoice = 'none' | 'fade' | 'pop' | 'slideUp' | 'typewriter';
export type OutChoice = 'none' | 'fade' | 'pop' | 'slideDown';

const PRESETS: Record<Exclude<InChoice | OutChoice, 'none'>, { word: string; from?: string }> = {
  fade: { word: 'fade' },
  pop: { word: 'pop' },
  slideUp: { word: 'slide', from: 'bottom' },
  slideDown: { word: 'slide', from: 'bottom' },
  typewriter: { word: 'typewriter' },
};

/** The choice a layer's enter (or exit) is: a preset the inspector offers, else the nearest it names, else none. */
export function animationOf(motion: Motion | undefined, end: 'enter' | 'exit'): InChoice | OutChoice | null {
  const use = motion?.[end];
  if (!use) return 'none';
  const word = use.preset.split('/').pop()?.replace('motion.', '');
  if (word === 'slide') return end === 'enter' ? 'slideUp' : 'slideDown';
  if (word === 'fade' || word === 'pop') return word;
  if (word === 'typewriter' && end === 'enter') return 'typewriter';
  // Set from chat: shown as it is, not one of the choices.
  return null;
}

/**
 * `layout`'s motion with `end` set to `choice` (or cleared by "none"), keeping the rest of it; undefined when nothing is
 * left. `durationUs`: the layer's length (an animation never runs longer).
 */
export function withAnimation(
  layout: Layout,
  end: 'enter' | 'exit',
  choice: InChoice | OutChoice,
  durationUs: number,
): Motion | undefined {
  const motion: Motion = { ...layout.motion };
  if (choice === 'none') {
    delete motion[end];
  } else {
    const { word, from } = PRESETS[choice];
    motion[end] = {
      preset: builtinId(`motion.${word}`),
      params: from ? { from } : {},
      durationUs: Math.min(ANIMATION_US, durationUs),
    };
  }
  const left = motion.enter ?? motion.exit ?? motion.loop ?? motion.keys;
  return left === undefined ? undefined : motion;
}

/** A text's background choice: none, a box behind the words, or a shadow under them. */
export type Backdrop = 'none' | 'box' | 'shadow';

/** The box behind the words: the design's navy at 86 %. */
export const BOX_COLOR = '#121A33DB';

export function backdropOf(style: TextStyle): Backdrop {
  if (style.background) return 'box';
  return style.shadow ? 'shadow' : 'none';
}

export function backdropStyle(style: TextStyle, backdrop: Backdrop): TextStyle {
  if (backdrop === 'box') return { ...style, background: style.background ?? BOX_COLOR, shadow: false };
  if (backdrop === 'shadow') return { ...style, background: null, shadow: true };
  return { ...style, background: null, shadow: false };
}

/** Kept clear around a layer placed at a grid position: 4 % of the shorter side. */
const MARGIN = 0.04;

/**
 * The box `rect` (in `format`) moved to grid position `anchor`, `MARGIN` in from the edges (in 9:16, from the safe
 * area's edges), its size kept.
 */
export function placedAt(rect: BoxN, anchor: Anchor, format: FormatKind, canvas: [number, number]): BoxN {
  const [w, h] = canvas;
  const short = Math.min(w, h);
  const area = safeRect(format, 'all') ?? { x: 0, y: 0, w: 1, h: 1 };
  const mx = (MARGIN * short) / w;
  const my = (MARGIN * short) / h;
  const left = area.x + mx;
  const top = area.y + my;
  const right = area.x + area.w - mx - rect.w;
  const bottom = area.y + area.h - my - rect.h;
  const col = anchor.endsWith('Left') ? 0 : anchor.endsWith('Right') ? 2 : 1;
  const row = anchor.startsWith('top') ? 0 : anchor.startsWith('bottom') ? 2 : 1;
  const x = col === 0 ? left : col === 2 ? right : area.x + (area.w - rect.w) / 2;
  const y = row === 0 ? top : row === 2 ? bottom : area.y + (area.h - rect.h) / 2;
  return { ...rect, x, y };
}

/** The layout placed at grid position `anchor` in `format` (an override in a format other than the primary). */
export function positionedLayout(
  layout: Layout,
  anchor: Anchor,
  format: FormatKind,
  primary: FormatKind,
  canvas: [number, number],
): Layout {
  const rect = placedAt(resolveRect(layout, primary, format), anchor, format, canvas);
  const placed = layoutPatchFor(layout, rect, format, primary);
  return format === primary ? { ...placed, anchor } : placed;
}

/** The grid position a layout's box is nearest in `format`. */
export function positionOf(layout: Layout, format: FormatKind, primary: FormatKind): Anchor {
  return anchorForRect(resolveRect(layout, primary, format));
}

/** "0:03.0", "1:05", "3.5" (seconds) or "1:02:03" as microseconds; null when it isn't a time. */
export function parseTime(text: string): number | null {
  const parts = text.trim().replace(/s$/i, '').trim().split(':');
  if (parts.length === 0 || parts.length > 3 || parts.some((p) => p.trim() === '' || !/^\d+(\.\d+)?$/.test(p.trim())))
    return null;
  const seconds = parts.reduce((total, part) => total * 60 + Number(part), 0);
  return Number.isFinite(seconds) ? Math.round(seconds * 1_000_000) : null;
}
