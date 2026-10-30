export interface Size {
  width: number;
  height: number;
}

/**
 * Device pixels for a preview box: CSS size times the device pixel ratio, scaled down to `maxPixels` and rounded down
 * to even numbers (video friendly), never below 2.
 */
export function previewPixels(css: Size, dpr: number, maxPixels: number): Size {
  let width = css.width * dpr;
  let height = css.height * dpr;
  if (width * height > maxPixels && maxPixels > 0) {
    const s = Math.sqrt(maxPixels / (width * height));
    width *= s;
    height *= s;
  }
  const even = (n: number) => Math.max(2, Math.floor(n / 2) * 2);
  return { width: even(width), height: even(height) };
}

/** The native preview's place: the box in page px, snapped to device pixels so the layer and the hole's edges meet. */
export interface HoleRect {
  x: number;
  y: number;
  width: number;
  height: number;
  scale: number;
}

export function holeRect(el: Element, scale: number): HoleRect | null {
  const r = el.getBoundingClientRect();
  if (r.width <= 0 || r.height <= 0) return null;
  const snap = (v: number) => Math.round(v * scale) / scale;
  const x = snap(r.left);
  const y = snap(r.top);
  return { x, y, width: snap(r.right) - x, height: snap(r.bottom) - y, scale };
}

/** Each format's shape, width to height. */
export const ASPECT = {
  landscape: [16, 9],
  portrait: [9, 16],
  square: [1, 1],
} as const;
