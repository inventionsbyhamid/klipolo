import { describe, expect, it } from 'vitest';
import { clockAnchor, clockUs, holdClock, reportFrame, setClock } from './playClock';

describe('playback clock', () => {
  it('starts with the first frame and re-anchors only when it drifts by more than a frame', () => {
    holdClock(2_000_000);
    expect(clockUs(5_000)).toBe(2_000_000);
    // The first playback frame starts it.
    expect(reportFrame(2_000_000, 1_000)).toBe(true);
    expect(clockUs(1_500)).toBe(2_500_000);
    // A frame 20 ms off (delivery jitter): the playhead's animation keeps running.
    expect(reportFrame(3_020_000, 2_000)).toBe(false);
    expect(clockAnchor().atMs).toBe(1_000);
    // 50 ms off (the sound clock moved): re-anchored there.
    expect(reportFrame(4_050_000, 3_000)).toBe(true);
    expect(clockUs(3_100)).toBe(4_150_000);
    holdClock(4_000_000);
    expect(clockUs(9_000)).toBe(4_000_000);
  });

  it('runs at the anchor rate, backwards too', () => {
    setClock({ us: 10_000_000, atMs: 1_000, running: true, rate: -2 });
    expect(clockUs(1_500)).toBe(9_000_000);
  });
});
