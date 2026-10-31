import type { BoxN } from '../../bindings/BoxN';

/** Off, every platform's combined area, or one app's: the 9:16 safe area a drag shows and snaps to. */
export type SafeAreaId = 'off' | 'all' | 'tiktok' | 'reels' | 'shorts';

/** One piece of an app's own UI over the video, in fractions of the 9:16 frame. */
export interface SafeShape {
  kind: 'buttons' | 'caption' | 'tabs' | 'nav';
  rect: BoxN;
  /** A circle (buttons) rather than a rounded rectangle. */
  round: boolean;
}

export interface SafeAreaSpec {
  id: Exclude<SafeAreaId, 'off'>;
  /** Where nothing of the app covers the video (snap targets). */
  clear: BoxN;
  /** The app's own UI, drawn as a silhouette. Approximate: compare them with a phone by hand. */
  shapes: SafeShape[];
}

/** Insets in pixels at 1080 × 1920 → the clear box. */
export function clearFrom(top: number, bottom: number, left: number, right: number): BoxN {
  return { x: left / 1080, y: top / 1920, w: 1 - (left + right) / 1080, h: 1 - (top + bottom) / 1920 };
}

/** A column of round buttons at `x`, one at each of `ys` (0.06 × 0.034 of the frame is a circle in 9:16). */
export function buttonColumn(x: number, ys: number[]): SafeShape[] {
  return ys.map((y) => ({ kind: 'buttons', rect: { x, y, w: 0.06, h: 0.034 }, round: true }));
}
