import { buttonColumn, clearFrom, type SafeAreaSpec } from './spec';

/**
 * YouTube Shorts, organic (Adaptly, 2026): 120 top, 300 bottom, 0 left, 96 right at 1080 × 1920
 * (360 bottom with the description expanded).
 */
export const SHORTS: SafeAreaSpec = {
  id: 'shorts',
  clear: clearFrom(120, 300, 0, 96),
  shapes: [
    // Search, more and the camera at the top right.
    { kind: 'tabs', rect: { x: 0.72, y: 0.025, w: 0.25, h: 0.035 }, round: false },
    ...buttonColumn(0.9, [0.5, 0.58, 0.66, 0.74, 0.82, 0.9]),
    { kind: 'caption', rect: { x: 0.04, y: 0.85, w: 0.76, h: 0.06 }, round: false },
    { kind: 'nav', rect: { x: 0, y: 0.95, w: 1, h: 0.05 }, round: false },
  ],
};
