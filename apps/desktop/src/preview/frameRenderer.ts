import { FORMAT_NV12, type FrameMeta } from './frameTrailer';
import { createWebGL2, nv12ToRgba, sizeCanvas, type FrameDrawer } from './webglFrames';

export type FrameRenderer = FrameDrawer;

/** A lost WebGL context that isn't restored within this long falls back to Canvas2D. */
const RESTORE_WAIT_MS = 2000;

function createCanvas2D(canvas: HTMLCanvasElement): FrameRenderer {
  const ctx = canvas.getContext('2d', { alpha: false, desynchronized: true });
  return {
    kind: 'canvas2d',
    draw(buf, meta) {
      if (!ctx) return;
      sizeCanvas(canvas, meta);
      const row = meta.width * 4;
      let data: Uint8ClampedArray<ArrayBuffer>;
      if (meta.format === FORMAT_NV12) {
        data = new Uint8ClampedArray(row * meta.height);
        nv12ToRgba(buf, meta, data);
      } else if (meta.stride === row) {
        data = new Uint8ClampedArray(buf, 0, row * meta.height);
      } else {
        data = new Uint8ClampedArray(row * meta.height);
        for (let y = 0; y < meta.height; y++) data.set(new Uint8Array(buf, y * meta.stride, row), y * row);
      }
      ctx.putImageData(new ImageData(data, meta.width, meta.height), 0, 0);
    },
    dispose() {},
  };
}

/** A fresh canvas filling `host` (a canvas that ever had a WebGL context can't give a 2D one). */
function mountCanvas(host: HTMLElement): HTMLCanvasElement {
  const canvas = document.createElement('canvas');
  canvas.className = 'frame-canvas';
  host.replaceChildren(canvas);
  return canvas;
}

/** WebGL2, or null when the canvas can't give it a working context (none, or a shader that won't compile). */
function tryWebGL2(canvas: HTMLCanvasElement): FrameRenderer | null {
  try {
    return createWebGL2(canvas);
  } catch (error) {
    console.warn('WebGL2 preview unavailable; drawing with Canvas2D', error);
    return null;
  }
}

/**
 * Draws engine frames into a canvas it creates inside `host`: WebGL2 when available, Canvas2D otherwise. A lost
 * WebGL context is restored once; a second loss, or a loss that isn't restored, switches to Canvas2D for good. The
 * last frame is drawn again after a restore or a switch, so a paused preview never goes black.
 */
export function createRenderer(host: HTMLElement): FrameRenderer {
  let losses = 0;
  let canvas = mountCanvas(host);
  let last: [ArrayBuffer, FrameMeta] | null = null;
  let restoreTimer: ReturnType<typeof setTimeout> | undefined;
  const redraw = () => {
    if (last) current.draw(...last);
  };

  const listen = (on: boolean) => {
    if (on) {
      canvas.addEventListener('webglcontextlost', onLost);
      canvas.addEventListener('webglcontextrestored', onRestored);
    } else {
      canvas.removeEventListener('webglcontextlost', onLost);
      canvas.removeEventListener('webglcontextrestored', onRestored);
    }
  };
  // A canvas that ever had a WebGL context can't give a 2D one, so Canvas2D always gets a fresh canvas.
  const fallBack = () => {
    clearTimeout(restoreTimer);
    listen(false);
    canvas = mountCanvas(host);
    current = createCanvas2D(canvas);
    redraw();
  };
  function onLost(e: Event) {
    e.preventDefault(); // allows the browser to restore the context
    losses += 1;
    if (losses >= 2) return fallBack();
    restoreTimer = setTimeout(fallBack, RESTORE_WAIT_MS);
  }
  function onRestored() {
    clearTimeout(restoreTimer);
    const restored = tryWebGL2(canvas);
    if (!restored) return fallBack();
    current = restored;
    redraw();
  }

  let current: FrameRenderer = tryWebGL2(canvas) ?? createCanvas2D((canvas = mountCanvas(host)));
  if (current.kind === 'webgl2') listen(true);

  return {
    get kind() {
      return current.kind;
    },
    draw: (buf, meta) => {
      last = [buf, meta];
      current.draw(buf, meta);
    },
    dispose() {
      clearTimeout(restoreTimer);
      listen(false);
      current.dispose();
      canvas.remove();
    },
  };
}
