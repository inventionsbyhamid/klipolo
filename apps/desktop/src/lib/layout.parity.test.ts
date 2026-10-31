import { describe, expect, it } from 'vitest';
import type { BoxN } from '../bindings/BoxN';
import type { CameraLayout } from '../bindings/CameraLayout';
import type { FormatKind } from '../bindings/FormatKind';
import type { Item } from '../bindings/Item';
import type { Layout } from '../bindings/Layout';
import { cameraRect, containRect, fullFrame, resolveRect, textScale } from './layout';
import parity from './layout-parity.json';
import { safeRect } from './safeAreas';
import { shownFromUs } from './timeline';

interface Fixture {
  rects: { layout: Layout; primary: FormatKind; target: FormatKind; rect: BoxN }[];
  cameras: {
    camera: CameraLayout;
    padding: number;
    primary: FormatKind;
    format: FormatKind;
    rect: BoxN | null;
  }[];
  contains: { rect: BoxN; width: number; height: number; format: FormatKind; box: BoxN }[];
  texts: { layout: Layout; primary: FormatKind; format: FormatKind; scale: number }[];
  shown: { item: Item; shownUs: number }[];
  fullFrame: Layout;
  portraitClear: BoxN;
}
const fixture = parity as unknown as Fixture;

function same(a: BoxN | null, b: BoxN | null) {
  if (a === null || b === null) return expect(a).toBe(b);
  for (const k of ['x', 'y', 'w', 'h'] as const) expect(Math.abs(a[k] - b[k])).toBeLessThan(1e-9);
}

describe('layout parity with the engine', () => {
  it('resolves layer rectangles exactly like Rust', () => {
    expect(fixture.rects.length).toBe(81);
    for (const c of fixture.rects) same(resolveRect(c.layout, c.primary, c.target), c.rect);
  });

  it('places the camera exactly like Rust', () => {
    expect(fixture.cameras.length).toBe(54);
    for (const c of fixture.cameras) same(cameraRect(c.camera, c.padding, c.primary, c.format), c.rect);
  });

  it('fits pictures in their boxes exactly like Rust', () => {
    expect(fixture.contains.length).toBe(24);
    for (const c of fixture.contains) same(containRect(c.rect, c.width, c.height, c.format), c.box);
  });

  it('scales text with its box per format exactly like Rust', () => {
    expect(fixture.texts.length).toBe(27);
    expect(fixture.texts.some((c) => Math.abs(c.scale - 1) > 0.01)).toBe(true);
    for (const c of fixture.texts)
      expect(Math.abs(textScale(c.layout, c.primary, c.format) - c.scale)).toBeLessThan(1e-9);
  });

  it('shows a layer whole from the same time as Rust (past its enter preset)', () => {
    expect(fixture.shown.length).toBe(6);
    for (const c of fixture.shown) expect(shownFromUs(c.item)).toBe(c.shownUs);
  });

  it('fills the frame in every format exactly like Rust (title templates)', () => {
    const layout = fullFrame();
    expect(layout).toEqual(fixture.fullFrame);
    // Each box its own: a drag that changes one in place leaves the others.
    expect(new Set([layout.rect, ...Object.values(layout.overrides)]).size).toBe(4);
  });

  it('keeps 9:16 text where the apps leave clear, the safe area the preview draws', () => {
    expect(safeRect('portrait', 'all')).toEqual(fixture.portraitClear);
  });
});
