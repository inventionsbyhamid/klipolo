import { act, render } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { EngineInfo } from '../bindings/EngineInfo';
import { FLAG_PLAYBACK, type FrameMeta } from './frameTrailer';
import { presentDelay } from './presentDelay';

/*
 * Sound and picture line up on the socket presenter: the page measures how long a playing frame takes from the engine
 * sending it to arriving here, and about once a second tells the engine the median, which then renders frames that far
 * ahead of the sound.
 */

vi.mock('../bridge/engine', () => ({ engine: { presentDelay: vi.fn(() => Promise.resolve()) } }));
let deliver: (buf: ArrayBuffer, meta: FrameMeta) => void = () => undefined;
vi.mock('../bridge/frameSocket', () => ({
  openFrameSocket: (_info: unknown, onFrame: typeof deliver) => {
    deliver = onFrame;
    return { close: () => undefined };
  },
}));
vi.mock('./frameRenderer', () => ({ createRenderer: () => ({ draw: () => undefined, dispose: () => undefined }) }));
const { engine } = await import('../bridge/engine');
const { useEngine } = await import('../state/engine');
const { SocketCanvas } = await import('./SocketCanvas');

/** A frame sent `ago` ms before now, playing or a still. */
const frame = (ago: number, playing = true): FrameMeta => ({
  width: 640,
  height: 360,
  stride: 2560,
  format: 0,
  flags: playing ? FLAG_PLAYBACK : 0,
  seq: 1,
  session: 1,
  revision: 1,
  timeUs: 0,
  sentMs: Date.now() - ago,
  dueMs: 0,
});

beforeEach(() => {
  vi.useFakeTimers({ toFake: ['Date'] });
  vi.setSystemTime(1_000_000);
  vi.clearAllMocks();
});
afterEach(() => vi.useRealTimers());

describe('the present delay', () => {
  it('is the median of ten playing frames, then waits a second and ten more', () => {
    const note = presentDelay();
    const delays = [30, 31, 29, 30, 400, 32, 28, 30, 31, 30];
    expect(delays.slice(0, 9).map((ago) => note(frame(ago)))).toEqual(Array(9).fill(null));
    // An outlier doesn't pull it: the median, not the mean.
    expect(note(frame(delays[9]))).toBe(30);
    // The next ten within the second wait for it.
    for (let i = 0; i < 12; i++) expect(note(frame(50))).toBeNull();
    vi.setSystemTime(1_001_000);
    expect(note(frame(50))).toBe(50);
  });

  it('counts playing frames only: a still or a seek never does', () => {
    const note = presentDelay();
    for (let i = 0; i < 20; i++) expect(note(frame(80, false))).toBeNull();
    for (let i = 0; i < 9; i++) note(frame(20));
    expect(note(frame(20))).toBe(20);
  });

  it('starts afresh after a pause: frames from before it never join the next median', () => {
    const note = presentDelay();
    for (let i = 0; i < 9; i++) note(frame(400));
    // A seek's still while paused, then playing again.
    expect(note(frame(0, false))).toBeNull();
    for (let i = 0; i < 9; i++) expect(note(frame(30))).toBeNull();
    expect(note(frame(30))).toBe(30);
  });

  it('the socket preview tells the engine while it plays', () => {
    useEngine.setState({ session: 1 });
    const info: EngineInfo = {
      port: 4321,
      token: 't',
      maxPreviewPixels: 1920 * 1080,
      adapter: 'test',
      presenter: 'socket',
      gpuClass: 'integrated',
    };
    render(<SocketCanvas info={info} aspect={[16, 9]} />);
    act(() => {
      for (let i = 0; i < 10; i++) deliver(new ArrayBuffer(8), frame(42));
    });
    expect(engine.presentDelay).toHaveBeenCalledTimes(1);
    expect(engine.presentDelay).toHaveBeenCalledWith(42);
  });
});
