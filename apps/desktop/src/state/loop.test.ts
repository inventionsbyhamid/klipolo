import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { fixtureSnapshot } from '../test/fixtures';

vi.mock('../bridge/engine', () => ({
  engine: {
    seek: vi.fn(() => Promise.resolve()),
    play: vi.fn(() => Promise.resolve()),
    pause: vi.fn(() => Promise.resolve(0)),
  },
}));

const { engine } = await import('../bridge/engine');
const { useProject } = await import('./project');
const { usePlayback } = await import('./playback');
const { holdClock, reportFrame } = await import('./playClock');
const { useRange } = await import('./range');
const { keepLooping, useLoop } = await import('./loop');

let stop = () => undefined as void;

beforeEach(() => {
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout', 'performance', 'requestAnimationFrame'] });
  vi.mocked(engine.play).mockClear();
  vi.mocked(engine.seek).mockClear();
  // The fixture's video is 30 s long, at 30 fps.
  useProject.setState({ snapshot: fixtureSnapshot(), pending: 0 });
  usePlayback.setState({ playheadUs: 0, playing: false, rate: 1, seq: 4 });
  holdClock(0);
  useRange.getState().clear();
  useLoop.setState({ on: false });
});

afterEach(() => {
  stop();
  vi.useRealTimers();
});

describe('loop playback', () => {
  it('is off until turned on, and the button or ⌘L turns it on and off again', () => {
    expect(useLoop.getState().on).toBe(false);
    useLoop.getState().toggle();
    expect(useLoop.getState().on).toBe(true);
    useLoop.getState().toggle();
    expect(useLoop.getState().on).toBe(false);
  });

  it('plays the whole video again from the start when it reaches the end', () => {
    useLoop.setState({ on: true });
    usePlayback.setState({ playheadUs: 29_000_000, playing: true, rate: 1 });
    usePlayback.getState().onState({ playing: false, atUs: 29_966_667, reason: 'ended', rate: 1 });
    expect(usePlayback.getState()).toMatchObject({ playing: true, rate: 1, playheadUs: 0 });
    expect(engine.play).toHaveBeenLastCalledWith(0, 1, usePlayback.getState().seq);
  });

  it('keeps its speed when it starts over (L L, then the end)', () => {
    useLoop.setState({ on: true });
    usePlayback.setState({ playheadUs: 29_000_000, playing: true, rate: 2 });
    usePlayback.getState().onState({ playing: false, atUs: 29_966_667, reason: 'ended', rate: 2 });
    expect(engine.play).toHaveBeenLastCalledWith(0, 2, usePlayback.getState().seq);
  });

  it('stops at the end as before while it is off, and never loops a stop for another reason', () => {
    usePlayback.setState({ playheadUs: 29_000_000, playing: true, rate: 1 });
    usePlayback.getState().onState({ playing: false, atUs: 29_966_667, reason: 'ended', rate: 1 });
    expect(usePlayback.getState()).toMatchObject({ playing: false, playheadUs: 29_966_667 });
    useLoop.setState({ on: true });
    usePlayback.setState({ playing: true });
    usePlayback.getState().onState({ playing: false, atUs: 12_000_000, reason: 'hidden', rate: 1 });
    expect(usePlayback.getState()).toMatchObject({ playing: false, playheadUs: 12_000_000 });
    expect(engine.play).not.toHaveBeenCalled();
  });

  it('backward, the start is still the end of the run', () => {
    useLoop.setState({ on: true });
    usePlayback.setState({ playheadUs: 1_000_000, playing: true, rate: -1 });
    usePlayback.getState().onState({ playing: false, atUs: 0, reason: 'ended', rate: -1 });
    expect(usePlayback.getState()).toMatchObject({ playing: false, playheadUs: 0 });
    expect(engine.play).not.toHaveBeenCalled();
  });

  it('with a time range selected, play starts at its start when the playhead is outside it', () => {
    useLoop.setState({ on: true });
    useRange.getState().select(5_000_000, 8_000_000);
    usePlayback.setState({ playheadUs: 20_000_000 });
    usePlayback.getState().play();
    expect(usePlayback.getState().playheadUs).toBe(5_000_000);
    expect(engine.play).toHaveBeenLastCalledWith(5_000_000, 1, 5);
    // Inside the range it plays from the playhead.
    usePlayback.setState({ playing: false, playheadUs: 6_000_000 });
    usePlayback.getState().play();
    expect(engine.play).toHaveBeenLastCalledWith(6_000_000, 1, 5);
    // L from outside the range starts there too.
    usePlayback.setState({ playing: false, playheadUs: 1_000_000 });
    usePlayback.getState().shuttle(1);
    expect(engine.play).toHaveBeenLastCalledWith(5_000_000, 1, 6);
  });

  it('a run asked to play from where it is (a moment chat points at) starts there, outside the range too', () => {
    useLoop.setState({ on: true });
    useRange.getState().select(5_000_000, 8_000_000);
    usePlayback.setState({ playheadUs: 20_000_000 });
    usePlayback.getState().play(1, { here: true });
    expect(engine.play).toHaveBeenLastCalledWith(20_000_000, 1, 4);
  });

  it('a seek the run replaces is never sent after it: the run starts at the range', () => {
    useLoop.setState({ on: true });
    useRange.getState().select(5_000_000, 8_000_000);
    usePlayback.getState().seek(20_000_000);
    usePlayback.getState().play();
    vi.advanceTimersToNextFrame();
    expect(engine.seek).not.toHaveBeenCalled();
    expect(engine.play).toHaveBeenLastCalledWith(5_000_000, 1, usePlayback.getState().seq);
  });

  it('while it is off, play starts at the playhead whatever is selected', () => {
    useRange.getState().select(5_000_000, 8_000_000);
    usePlayback.setState({ playheadUs: 20_000_000 });
    usePlayback.getState().play();
    expect(engine.play).toHaveBeenLastCalledWith(20_000_000, 1, 4);
  });

  it('goes back to the range start each time the clock reaches its end', () => {
    stop = keepLooping();
    useLoop.setState({ on: true });
    useRange.getState().select(5_000_000, 8_000_000);
    usePlayback.setState({ playheadUs: 6_000_000, playing: true, rate: 1 });
    reportFrame(6_000_000, performance.now(), 1);
    vi.advanceTimersByTime(1_900);
    expect(usePlayback.getState().playheadUs).toBe(6_000_000);
    vi.advanceTimersByTime(150);
    expect(usePlayback.getState().playheadUs).toBe(5_000_000);
    vi.advanceTimersToNextFrame();
    expect(engine.seek).toHaveBeenLastCalledWith(5_000_000, usePlayback.getState().seq);
    // The run goes on from there: the next time round goes back again.
    reportFrame(5_000_000, performance.now(), 1);
    vi.advanceTimersByTime(3_050);
    expect(usePlayback.getState().playheadUs).toBe(5_000_000);
    expect(engine.seek).toHaveBeenCalledTimes(2);
  });

  it('at double speed it gets there twice as soon', () => {
    stop = keepLooping();
    useLoop.setState({ on: true });
    useRange.getState().select(5_000_000, 8_000_000);
    usePlayback.setState({ playheadUs: 6_000_000, playing: true, rate: 2 });
    reportFrame(6_000_000, performance.now(), 2);
    vi.advanceTimersByTime(1_050);
    expect(usePlayback.getState().playheadUs).toBe(5_000_000);
  });

  it('turned on mid-run, it goes back once the clock reaches the range end', () => {
    stop = keepLooping();
    useRange.getState().select(5_000_000, 8_000_000);
    usePlayback.setState({ playheadUs: 6_000_000, playing: true, rate: 1 });
    reportFrame(6_000_000, performance.now(), 1);
    vi.advanceTimersByTime(1_000);
    useLoop.setState({ on: true });
    vi.advanceTimersByTime(1_050);
    expect(usePlayback.getState().playheadUs).toBe(5_000_000);
  });

  it("doesn't go back while it is off, paused, going backward, past the range, or once the range is cleared", () => {
    stop = keepLooping();
    useRange.getState().select(5_000_000, 8_000_000);
    usePlayback.setState({ playheadUs: 6_000_000, playing: true, rate: 1 });
    reportFrame(6_000_000, performance.now(), 1);
    vi.advanceTimersByTime(3_000);
    expect(usePlayback.getState().playheadUs).toBe(6_000_000);
    // Turned on with the clock past the range's end (9 s): it plays on, to the video's end.
    useLoop.setState({ on: true });
    vi.advanceTimersByTime(3_000);
    expect(engine.seek).not.toHaveBeenCalled();
    useRange.getState().clear();
    vi.advanceTimersByTime(3_000);
    expect(engine.seek).not.toHaveBeenCalled();
    useRange.getState().select(5_000_000, 20_000_000);
    holdClock(17_000_000);
    usePlayback.setState({ playing: false });
    vi.advanceTimersByTime(20_000);
    expect(engine.seek).not.toHaveBeenCalled();
    usePlayback.setState({ playheadUs: 19_000_000, playing: true, rate: -1 });
    reportFrame(19_000_000, performance.now(), -1);
    vi.advanceTimersByTime(20_000);
    expect(engine.seek).not.toHaveBeenCalled();
  });

  it('a frame that re-anchors the clock just past the end, before the timer fires, still goes back', () => {
    stop = keepLooping();
    useLoop.setState({ on: true });
    useRange.getState().select(5_000_000, 8_000_000);
    usePlayback.setState({ playheadUs: 7_900_000, playing: true, rate: 1 });
    reportFrame(7_900_000, performance.now(), 1);
    // Before the timer's turn, a frame drawn late re-anchors the clock past the end.
    reportFrame(8_040_000, performance.now() - 50, 1);
    expect(usePlayback.getState().playheadUs).toBe(5_000_000);
  });

  it('a range that ends with the video loops at the end, as the whole video does', () => {
    stop = keepLooping();
    useLoop.setState({ on: true });
    useRange.getState().select(25_000_000, 40_000_000);
    usePlayback.setState({ playheadUs: 29_000_000, playing: true, rate: 1 });
    reportFrame(29_000_000, performance.now(), 1);
    vi.advanceTimersByTime(5_000);
    // The engine's end, not a seek, takes it back.
    expect(engine.seek).not.toHaveBeenCalled();
    usePlayback.getState().onState({ playing: false, atUs: 29_966_667, reason: 'ended', rate: 1 });
    expect(engine.play).toHaveBeenLastCalledWith(25_000_000, 1, usePlayback.getState().seq);
  });
});
