/** How near the lanes' left or right edge (px) the pointer starts scrolling them, and the fastest step (px/frame). */
const EDGE_PX = 32;
const MAX_STEP_PX = 16;

/**
 * While a drag runs, scrolls `scroller` when the pointer (`pointerX()`) is within 32 px of the lanes' left edge (past
 * the `insetPx` header column) or right edge: up to 16 px a frame, faster the deeper it is, and calls `onScroll` in
 * the same frame so the dragged block stays under the pointer. Returns the stop. Does nothing without a layout (jsdom).
 */
export function startAutoscroll(
  scroller: HTMLElement,
  insetPx: number,
  pointerX: () => number,
  onScroll: () => void,
): () => void {
  const rect = scroller.getBoundingClientRect();
  if (rect.width === 0) return () => undefined;
  const left = rect.left + insetPx;
  const right = rect.left + (scroller.clientWidth || rect.width);
  let raf = 0;
  const tick = () => {
    raf = requestAnimationFrame(tick);
    const x = pointerX();
    const depth = x < left + EDGE_PX ? x - (left + EDGE_PX) : x > right - EDGE_PX ? x - (right - EDGE_PX) : 0;
    if (depth === 0) return;
    const step =
      Math.sign(depth) * Math.max(1, Math.round(MAX_STEP_PX * Math.min(1, Math.abs(depth) / EDGE_PX)));
    const before = scroller.scrollLeft;
    scroller.scrollLeft = before + step;
    if (scroller.scrollLeft !== before) onScroll();
  };
  raf = requestAnimationFrame(tick);
  return () => cancelAnimationFrame(raf);
}
