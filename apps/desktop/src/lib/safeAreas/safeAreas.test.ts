import { describe, expect, it } from 'vitest';
import { SAFE_AREAS, SAFE_AREA_IDS, dragArea, safeArea, safeRect } from './index';

const px = (v: number, of: number) => Math.round(v * of);

describe('safe areas', () => {
  it('uses each platform’s organic numbers at 1080 × 1920', () => {
    const tiktok = safeRect('portrait', 'tiktok')!;
    expect([
      px(tiktok.y, 1920),
      px(1 - tiktok.y - tiktok.h, 1920),
      px(tiktok.x, 1080),
      px(1 - tiktok.x - tiktok.w, 1080),
    ]).toEqual([108, 320, 60, 120]);
    const reels = safeRect('portrait', 'reels')!;
    expect([
      px(reels.y, 1920),
      px(1 - reels.y - reels.h, 1920),
      px(reels.x, 1080),
      px(1 - reels.x - reels.w, 1080),
    ]).toEqual([210, 310, 0, 84]);
    const shorts = safeRect('portrait', 'shorts')!;
    expect([
      px(shorts.y, 1920),
      px(1 - shorts.y - shorts.h, 1920),
      px(shorts.x, 1080),
      px(1 - shorts.x - shorts.w, 1080),
    ]).toEqual([120, 300, 0, 96]);
  });

  it('keeps all platforms as today’s combined area', () => {
    expect(safeRect('portrait', 'all')).toEqual({ x: 0.06, y: 0.14, w: 0.81, h: 0.6 });
  });

  it('has nothing off or outside 9:16, and a drag shows the last choice or all', () => {
    expect(safeRect('portrait', 'off')).toBeNull();
    expect(safeRect('landscape', 'tiktok')).toBeNull();
    expect(safeArea('off')).toBeNull();
    expect(dragArea('off', 'off')).toBe('all');
    expect(dragArea('off', 'reels')).toBe('reels');
    expect(dragArea('shorts', 'reels')).toBe('shorts');
  });

  it('registers each area once, under its own word, in the menu’s order', () => {
    expect(SAFE_AREA_IDS).toEqual(['off', 'all', 'tiktok', 'reels', 'shorts']);
    for (const id of SAFE_AREA_IDS.slice(1)) expect(safeArea(id)?.id).toBe(id);
  });

  it('draws every shape inside the frame and outside the clear area', () => {
    for (const spec of SAFE_AREAS) {
      for (const s of spec.shapes) {
        expect(
          s.rect.x >= 0 && s.rect.y >= 0 && s.rect.x + s.rect.w <= 1 && s.rect.y + s.rect.h <= 1,
          `${spec.id} ${s.kind} in the frame`,
        ).toBe(true);
        const c = spec.clear;
        const inside =
          s.rect.x >= c.x &&
          s.rect.y >= c.y &&
          s.rect.x + s.rect.w <= c.x + c.w &&
          s.rect.y + s.rect.h <= c.y + c.h;
        expect(inside, `${spec.id} ${s.kind}`).toBe(false);
      }
      // No two shapes overlap, so each is shaded once (the overlay has no group opacity).
      spec.shapes.forEach((a, i) =>
        spec.shapes.slice(i + 1).forEach((b) => {
          const apart =
            a.rect.x + a.rect.w <= b.rect.x ||
            b.rect.x + b.rect.w <= a.rect.x ||
            a.rect.y + a.rect.h <= b.rect.y ||
            b.rect.y + b.rect.h <= a.rect.y;
          expect(apart, `${spec.id}: ${a.kind} and ${b.kind}`).toBe(true);
        }),
      );
    }
  });
});
