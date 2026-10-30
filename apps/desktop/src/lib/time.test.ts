import { describe, expect, it } from 'vitest';
import { frameIndexAt, frameStartUs, lastFrameUs, stepFrames } from './time';

const RATES = [30, 60, 30_000 / 1_001];
const DURATION_US = 3_000_000;

/** Every playhead a pause, a click or a drag can leave: each frame's start, just after it, its middle and its end. */
function playheads(fps: number): number[] {
  const last = frameIndexAt(lastFrameUs(DURATION_US, fps), fps);
  const out: number[] = [];
  for (let n = 0; n <= last; n++) {
    const start = frameStartUs(n, fps);
    const next = frameStartUs(n + 1, fps);
    out.push(start, start + 1, Math.floor((start + next) / 2), next - 1);
  }
  return out;
}

describe('frame steps', () => {
  it('mirror the engine: frame 1 at 30 fps starts at 33 333, frame 2 at 66 667', () => {
    expect([frameStartUs(1, 30), frameStartUs(2, 30)]).toEqual([33_333, 66_667]);
    expect([frameIndexAt(66_666, 30), frameIndexAt(66_667, 30)]).toEqual([1, 2]);
    // The steps that stuck and skipped with "± frameUs".
    expect(stepFrames(33_333, 1, 30, DURATION_US)).toBe(66_667);
    expect(stepFrames(33_333, -1, 60, DURATION_US)).toBe(16_667);
  });

  for (const fps of RATES) {
    it(`visit every frame once, forward and back, from any playhead at ${fps.toFixed(2)} fps`, () => {
      const last = frameIndexAt(lastFrameUs(DURATION_US, fps), fps);
      for (const from of playheads(fps)) {
        const start = frameIndexAt(from, fps);
        for (const direction of [1, -1]) {
          const seen: number[] = [];
          let at = from;
          for (;;) {
            const next = stepFrames(at, direction, fps, DURATION_US);
            // Always a frame's start; at the first or last frame it stays on that frame.
            expect(next).toBe(frameStartUs(frameIndexAt(next, fps), fps));
            if (frameIndexAt(next, fps) === frameIndexAt(at, fps)) break;
            seen.push(frameIndexAt(next, fps));
            at = next;
          }
          const want =
            direction > 0
              ? Array.from({ length: last - start }, (_, i) => start + 1 + i)
              : Array.from({ length: start }, (_, i) => start - 1 - i);
          expect(seen, `${direction > 0 ? '→' : '←'} from ${from}`).toEqual(want);
        }
      }
    });
  }
});
