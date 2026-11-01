import type { ViewPixels } from '../../bindings/ViewPixels';

/**
 * A box's size in device pixels: its CSS size times the display's scale (`devicePixelRatio`; one it can't use reads as
 * 1), rounded. Null before it has a size.
 */
export function boxPixels(width: number, height: number, scale: number): ViewPixels | null {
  const by = scale > 0 ? scale : 1;
  const pixels = { width: Math.round(width * by), height: Math.round(height * by) };
  return pixels.width > 0 && pixels.height > 0 ? pixels : null;
}
