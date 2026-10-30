import { afterEach, describe, expect, it, vi } from 'vitest';
import { epochAt, perfAt } from './clocks';
import { FrameQueue, REFRESH_MS } from '../preview/framePick';
import type { FrameMeta } from '../preview/frameTrailer';

afterEach(() => vi.restoreAllMocks());

describe('clocks across a sleep', () => {
  it('compares engine wall-clock stamps with animation frames correctly after the Mac slept', () => {
    // An hour of sleep: the wall clock moved on, WebKit's performance clock (and its timeOrigin sum) didn't.
    const origin = 1_700_000_000_000;
    vi.spyOn(performance, 'now').mockReturnValue(5_000);
    vi.spyOn(Date, 'now').mockReturnValue(origin + 5_000 + 3_600_000);
    const rafAt = 4_990; // this animation frame began 10 ms ago
    const target = epochAt(rafAt) + REFRESH_MS;
    expect(target).toBeCloseTo(Date.now() - 10 + REFRESH_MS);
    // A playback frame due now (stamped by the engine's wall clock) is drawn on this refresh.
    const queue = new FrameQueue();
    queue.push({ buf: new ArrayBuffer(0), meta: { timeUs: 7, dueMs: Date.now() } as FrameMeta });
    expect(queue.take(target)?.meta.timeUs).toBe(7);
    // A native frame shown 8 ms from now, on the playback clock's performance time.
    expect(perfAt(Date.now() + 8)).toBe(5_008);
  });
});
