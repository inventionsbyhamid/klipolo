import type { MessageDescriptor } from '@lingui/core';
import { msg } from '@lingui/core/macro';

/** Text colours by name. These are content colours the user picks, not UI styling, so they're plain data. */
export const SWATCH = {
  White: '#FFFFFF',
  Navy: '#121A33',
  Yellow: '#FFD02E',
  Blue: '#7F92DA',
  Black: '#000000',
} as const;

/** Each colour's name as the swatch announces it. */
const NAMES: Record<keyof typeof SWATCH, MessageDescriptor> = {
  White: msg({ id: 'color.white', message: 'White' }),
  Navy: msg({ id: 'color.navy', message: 'Navy' }),
  Yellow: msg({ id: 'color.yellow', message: 'Yellow' }),
  Blue: msg({ id: 'color.blue', message: 'Blue' }),
  Black: msg({ id: 'color.black', message: 'Black' }),
};

/** The colours offered for text, highlights and outlines, in order. */
export const TEXT_SWATCHES: { name: MessageDescriptor; value: string }[] = (
  Object.keys(SWATCH) as (keyof typeof SWATCH)[]
).map((key) => ({ name: NAMES[key], value: SWATCH[key] }));

/** More colours to name a custom colour by (each round button is named by its colour). */
const MORE: [string, MessageDescriptor][] = [
  ['#FF453A', msg({ id: 'color.red', message: 'Red' })],
  ['#FF9F0A', msg({ id: 'color.orange', message: 'Orange' })],
  ['#32D74B', msg({ id: 'color.green', message: 'Green' })],
  ['#0A84FF', msg({ id: 'color.brightBlue', message: 'Bright blue' })],
  ['#FF375F', msg({ id: 'color.pink', message: 'Pink' })],
  ['#B8FF3C', msg({ id: 'color.lime', message: 'Lime' })],
  ['#7C4DFF', msg({ id: 'color.violet', message: 'Violet' })],
  ['#00F0FF', msg({ id: 'color.cyan', message: 'Cyan' })],
  ['#FF2BD6', msg({ id: 'color.magenta', message: 'Magenta' })],
  ['#92979E', msg({ id: 'color.gray', message: 'Gray' })],
  ['#A3895B', msg({ id: 'color.brown', message: 'Brown' })],
];

const NAMED: { hex: string; name: MessageDescriptor }[] = [
  ...TEXT_SWATCHES.map((s) => ({ hex: s.value, name: s.name })),
  ...MORE.map(([hex, name]) => ({ hex, name })),
];

const rgb = (hex: string) => [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16));

/** The name of the named colour nearest `hex` (`#RRGGBB`, an alpha left out), by distance in RGB. */
export function nearestColorName(hex: string): MessageDescriptor {
  const [r, g, b] = rgb(hex);
  let best = NAMED[0];
  let bestD = Infinity;
  for (const c of NAMED) {
    const [cr, cg, cb] = rgb(c.hex);
    const d = (r - cr) ** 2 + (g - cg) ** 2 + (b - cb) ** 2;
    if (d < bestD) [best, bestD] = [c, d];
  }
  return best.name;
}
