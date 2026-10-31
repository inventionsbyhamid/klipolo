import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('../bridge/engine', () => ({
  engine: {
    seek: vi.fn(() => Promise.resolve()),
    play: vi.fn(() => Promise.resolve()),
    pause: vi.fn(() => Promise.resolve(1_000_000)),
  },
}));

const { engine } = await import('../bridge/engine');
const { showFrameAt, usePlayback } = await import('./playback');

/** Animation frames by hand: what waits for the next one runs at `nextFrame()`. */
const waiting: FrameRequestCallback[] = [];
const nextFrame = () => waiting.splice(0).forEach((cb) => cb(0));
const seeksAfterPlay = () => {
  const played = Math.min(...vi.mocked(engine.play).mock.invocationCallOrder);
  return vi.mocked(engine.seek).mock.invocationCallOrder.filter((order) => order > played);
};

describe('a frame shown without moving the playhead, then playback', () => {
  beforeEach(() => {
    vi.stubGlobal('requestAnimationFrame', (cb: FrameRequestCallback) => waiting.push(cb));
    usePlayback.setState({ playheadUs: 1_000_000, playing: false, rate: 1, seq: 4 });
    vi.mocked(engine.seek).mockClear();
    vi.mocked(engine.play).mockClear();
  });
  afterEach(() => {
    nextFrame();
    vi.unstubAllGlobals();
  });

  it('play drops a shown frame still waiting, and plays from the playhead under a new sequence', () => {
    showFrameAt(9_000_000);
    usePlayback.getState().play();
    nextFrame();
    expect(engine.play).toHaveBeenCalledWith(1_000_000, 1, 6);
    expect(seeksAfterPlay()).toEqual([]);
    // Frames from the shown frame's sequence never move the playhead.
    usePlayback.getState().onFrame({ timeUs: 9_000_000, seq: 5, playback: true });
    expect(usePlayback.getState().playheadUs).toBe(1_000_000);
  });

  it('a shown frame already sent is older than the run, so the engine ignores it', () => {
    showFrameAt(9_000_000);
    nextFrame();
    expect(engine.seek).toHaveBeenLastCalledWith(9_000_000, 5);
    usePlayback.getState().play();
    expect(vi.mocked(engine.play).mock.calls[0][2]).toBeGreaterThan(5);
  });

  it('L does the same', () => {
    showFrameAt(9_000_000);
    usePlayback.getState().shuttle(1);
    nextFrame();
    expect(engine.play).toHaveBeenCalledWith(1_000_000, 1, 6);
    expect(seeksAfterPlay()).toEqual([]);
  });

  it("once the playhead's frame is shown again, play keeps its sequence", () => {
    showFrameAt(9_000_000);
    showFrameAt(null);
    usePlayback.getState().play();
    nextFrame();
    expect(engine.play).toHaveBeenCalledWith(1_000_000, 1, 6);
  });
});
