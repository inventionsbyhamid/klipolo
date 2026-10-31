import type { SafeAreaSpec } from './spec';

/**
 * The union of TikTok, Reels and Shorts chrome (the editor's guide): 14 % top, 26 % bottom, 6 % left and
 * 13 % right.
 */
export const ALL: SafeAreaSpec = {
  id: 'all',
  clear: { x: 0.06, y: 0.14, w: 0.81, h: 0.6 },
  shapes: [
    { kind: 'tabs', rect: { x: 0, y: 0, w: 1, h: 0.14 }, round: false },
    { kind: 'caption', rect: { x: 0, y: 0.74, w: 1, h: 0.26 }, round: false },
    { kind: 'buttons', rect: { x: 0.87, y: 0.14, w: 0.13, h: 0.6 }, round: false },
  ],
};
