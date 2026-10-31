export interface Size {
  width: number;
  height: number;
}

/**
 * How many of the preview's pixels (`previewPixels`, what the engine draws in) one CSS pixel of the `css` box is: the
 * screen's device pixel ratio, less where the pixel budget scales the preview down.
 */
export function previewScale(css: Size, dpr: number, maxPixels: number): number {
  return css.width > 0 ? previewPixels(css, dpr, maxPixels).width / css.width : 1;
}

/**
 * Device pixels for a preview box: CSS size × devicePixelRatio, scaled down to `maxPixels` and rounded down to
 * even numbers (video-friendly), never below 2.
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
