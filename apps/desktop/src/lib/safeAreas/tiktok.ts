import { buttonColumn, clearFrom, type SafeAreaSpec } from './spec';

/**
 * TikTok's feed, organic (Adaptly 2026): 108 top, 320 bottom, 60 left, 120 right at
 * 1080 × 1920.
 */
export const TIKTOK: SafeAreaSpec = {
  id: 'tiktok',
  clear: clearFrom(108, 320, 60, 120),
  shapes: [
    { kind: 'tabs', rect: { x: 0.25, y: 0.025, w: 0.5, h: 0.03 }, round: false },
    // The profile picture, a little larger than the buttons under it.
    { kind: 'buttons', rect: { x: 0.885, y: 0.44, w: 0.07, h: 0.04 }, round: true },
    ...buttonColumn(0.89, [0.52, 0.59, 0.66, 0.73, 0.8]),
    { kind: 'caption', rect: { x: 0.04, y: 0.84, w: 0.74, h: 0.07 }, round: false },
    { kind: 'nav', rect: { x: 0, y: 0.94, w: 1, h: 0.06 }, round: false },
  ],
};
