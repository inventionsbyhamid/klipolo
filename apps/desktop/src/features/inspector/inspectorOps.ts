import type { Anchor } from '../../bindings/Anchor';
import type { FormatKind } from '../../bindings/FormatKind';
import type { Layout } from '../../bindings/Layout';
import type { Motion } from '../../bindings/Motion';
import { builtinId } from '../../lib/contrib';
import { layoutPatchFor } from '../../lib/geometry';
import { FORMAT_SIZE, resolveRect } from '../../lib/layout';
import { placedAt } from '../../lib/placement';
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

/** The box behind the words (the Highlight field's navy, and the Box look's) lives with the other text looks. */
export { BOX_COLOR } from '../../lib/textLook';

/** The Position grid's places (`placedAt`) live in `lib/placement`, which places a new video layer at one too. */
export { placedAt };

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

const GRID: Anchor[][] = [
  ['topLeft', 'topCenter', 'topRight'],
  ['middleLeft', 'center', 'middleRight'],
  ['bottomLeft', 'bottomCenter', 'bottomRight'],
];
/** Within this of a grid place (a share of the frame), a box is at it. */
const AT = 1e-4;

/**
 * Which of an axis's three places (start, middle, end) a box at `at` is at; else the third of the area its centre
 * (`centre`, a share of the area) is in. A box as long as the area less the margins is at all three: the middle.
 */
function bandOf(at: number, places: [number, number, number], centre: number): 0 | 1 | 2 {
  const index = [1, 0, 2].find((k) => Math.abs(places[k] - at) < AT);
  if (index !== undefined) return index as 0 | 1 | 2;
  return centre < 1 / 3 ? 0 : centre < 2 / 3 ? 1 : 2;
}

/**
 * The grid position a layout's box is at in `format`: each way, the place `placedAt` puts it (so a layer placed with
 * the grid, or a new video layer, reads where it was put, in 9:16 too), else the third of the area its centre is in
 * (in 9:16, of the safe area, where the grid's places are).
 */
export function positionOf(layout: Layout, format: FormatKind, primary: FormatKind): Anchor {
  const rect = resolveRect(layout, primary, format);
  const canvas = FORMAT_SIZE[format];
  const [start, middle, end] = (['topLeft', 'center', 'bottomRight'] as const).map((cell) =>
    placedAt(rect, cell, format, canvas),
  );
  const area = safeRect(format, 'all') ?? { x: 0, y: 0, w: 1, h: 1 };
  const col = bandOf(rect.x, [start.x, middle.x, end.x], (rect.x + rect.w / 2 - area.x) / area.w);
  const row = bandOf(rect.y, [start.y, middle.y, end.y], (rect.y + rect.h / 2 - area.y) / area.h);
  return GRID[row][col];
}

/** "0:03.0", "1:05", "3.5" (seconds) or "1:02:03" as microseconds; null when it isn't a time. */
export function parseTime(text: string): number | null {
  const parts = text.trim().replace(/s$/i, '').trim().split(':');
  if (parts.length === 0 || parts.length > 3 || parts.some((p) => p.trim() === '' || !/^\d+(\.\d+)?$/.test(p.trim())))
    return null;
  const seconds = parts.reduce((total, part) => total * 60 + Number(part), 0);
  return Number.isFinite(seconds) ? Math.round(seconds * 1_000_000) : null;
}
