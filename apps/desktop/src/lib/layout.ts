import type { Anchor } from '../bindings/Anchor';
import type { BoxN } from '../bindings/BoxN';
import type { CameraLayout } from '../bindings/CameraLayout';
import type { CameraMode } from '../bindings/CameraMode';
import type { FormatKind } from '../bindings/FormatKind';
import type { Layout } from '../bindings/Layout';

export const FORMAT_SIZE: Record<FormatKind, [number, number]> = {
  landscape: [1920, 1080],
  portrait: [1080, 1920],
  square: [1080, 1080],
};

/** Short aspect-ratio label for each format, e.g. on format switches and project cards. */
export const FORMAT_SHORT: Record<FormatKind, string> = {
  landscape: '16:9',
  portrait: '9:16',
  square: '1:1',
};

type Band = 'start' | 'middle' | 'end';

function bands(anchor: Anchor): [Band, Band] {
  const col: Band = anchor.endsWith('Left') ? 'start' : anchor.endsWith('Right') ? 'end' : 'middle';
  const row: Band = anchor.startsWith('top') ? 'start' : anchor.startsWith('bottom') ? 'end' : 'middle';
  return [col, row];
}

function place(
  band: Band,
  pos: number,
  size: number,
  from: number,
  to: number,
  s: number,
  newSize: number,
): number {
  const placed =
    band === 'start'
      ? pos * s
      : band === 'middle'
        ? (pos + size / 2 - from / 2) * s + to / 2 - newSize / 2
        : to - (from - (pos + size)) * s - newSize;
  return Math.min(Math.max(placed, 0), Math.max(to - newSize, 0));
}

export function resolveRect(layout: Layout, primary: FormatKind, target: FormatKind): BoxN {
  const override = layout.overrides[target];
  if (override) return override;
  if (target === primary) return layout.rect;
  const [wp, hp] = FORMAT_SIZE[primary];
  const [wt, ht] = FORMAT_SIZE[target];
  const s = Math.min(wt, ht) / Math.min(wp, hp);
  const r = layout.rect;
  const wSrc = r.w * wp;
  const hSrc = r.h * hp;
  const wPx = Math.min(wSrc * s, wt);
  const hPx = Math.min(hSrc * s, ht);
  const [col, row] = bands(layout.anchor);
  return {
    x: place(col, r.x * wp, wSrc, wp, wt, s, wPx) / wt,
    y: place(row, r.y * hp, hSrc, hp, ht, s, hPx) / ht,
    w: wPx / wt,
    h: hPx / ht,
  };
}

export function cameraModeFor(camera: CameraLayout, format: FormatKind): CameraMode {
  return camera.modeOverrides[format] ?? camera.mode;
}

/** The canvas minus the background padding (a share of the shorter side, on every edge). Mirrors Rust `content_box`. */
export function contentBox(padding: number, format: FormatKind): BoxN {
  const [w, h] = FORMAT_SIZE[format];
  const inset = Math.min(Math.max(padding, 0), 0.4) * Math.min(w, h);
  return { x: inset / w, y: inset / h, w: (w - 2 * inset) / w, h: (h - 2 * inset) / h };
}

/** Where a recording's camera is drawn in `format`, or null when hidden there. Mirrors Rust `camera_rect_for`. */
export function cameraRect(
  camera: CameraLayout,
  padding: number,
  primary: FormatKind,
  format: FormatKind,
): BoxN | null {
  switch (cameraModeFor(camera, format)) {
    case 'bubble':
      return resolveRect(camera.bubble, primary, format);
    case 'splitBottom': {
      const c = contentBox(padding, format);
      return { x: c.x, y: c.y + c.h / 2, w: c.w, h: c.h / 2 };
    }
    default:
      return null;
  }
}

/** Nearest of the nine anchors to the rectangle's centre (mirrors Rust `anchor_for_rect`). */
export function anchorForRect(rect: BoxN): Anchor {
  const band = (v: number) => (v < 1 / 3 ? 0 : v < 2 / 3 ? 1 : 2);
  const grid = [
    ['topLeft', 'topCenter', 'topRight'],
    ['middleLeft', 'center', 'middleRight'],
    ['bottomLeft', 'bottomCenter', 'bottomRight'],
  ] as const;
  return grid[band(rect.y + rect.h / 2)][band(rect.x + rect.w / 2)];
}

/**
 * Where a picture of `width`×`height` lands inside its layout box in `format`: centred, keeping its shape, as the
 * engine draws image and video layers. An unknown size fills the box. Mirrors Rust `overlay::contain`.
 */
export function containRect(rect: BoxN, width: number, height: number, format: FormatKind): BoxN {
  const [cw, ch] = FORMAT_SIZE[format];
  const aw = rect.w * cw;
  const ah = rect.h * ch;
  const aspect = width === 0 || height === 0 ? aw / Math.max(ah, Number.EPSILON) : width / height;
  const [w, h] = aspect > aw / ah ? [aw, aw / aspect] : [ah * aspect, ah];
  return {
    x: (rect.x * cw + (aw - w) / 2) / cw,
    y: (rect.y * ch + (ah - h) / 2) / ch,
    w: w / cw,
    h: h / ch,
  };
}

/**
 * How much bigger a text layer's font is drawn in `format` than its size says: text scales with its box, so a format
 * whose box is taller in pixels (its own placement, or a derived one the canvas squeezes) draws it bigger. 1 in the
 * primary format. Mirrors Rust `overlay::text_scale` (parity-tested).
 */
export function textScale(layout: Layout, primary: FormatKind, format: FormatKind): number {
  const [pw, ph] = FORMAT_SIZE[primary];
  const [fw, fh] = FORMAT_SIZE[format];
  const base = resolveRect(layout, primary, primary).h * ph;
  if (!(base > 0)) return 1;
  return ((resolveRect(layout, primary, format).h * fh) / base) * (Math.min(pw, ph) / Math.min(fw, fh));
}
