import { buttonColumn, clearFrom, type SafeAreaSpec } from './spec';

/**
 * Instagram Reels, organic (Adaptly 2026): 210 top, 310 bottom, 0 left, 84 right at
 * 1080 × 1920.
 */
export const REELS: SafeAreaSpec = {
  id: 'reels',
  clear: clearFrom(210, 310, 0, 84),
  shapes: [
    // The "Reels" title and the camera at the top.
    { kind: 'tabs', rect: { x: 0.04, y: 0.04, w: 0.22, h: 0.03 }, round: false },
    { kind: 'buttons', rect: { x: 0.9, y: 0.04, w: 0.06, h: 0.034 }, round: true },
    ...buttonColumn(0.91, [0.6, 0.67, 0.74, 0.81, 0.88]),
    { kind: 'caption', rect: { x: 0.04, y: 0.85, w: 0.72, h: 0.06 }, round: false },
    { kind: 'nav', rect: { x: 0, y: 0.95, w: 1, h: 0.05 }, round: false },
  ],
};
