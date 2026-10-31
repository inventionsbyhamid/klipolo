import { act, fireEvent, screen } from '@testing-library/react';
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import { fixtureSnapshot } from '../../test/fixtures';
import { longProject } from '../../test/longProject';
import { renderWithProviders } from '../../test/render';

vi.mock('../../bridge/commands', () => ({ bridge: { apply: vi.fn() }, mediaUrl: (p: string) => p }));
vi.mock('../../bridge/engine', () => ({ engine: { seek: vi.fn(() => Promise.resolve()) } }));
const { useProject } = await import('../../state/project');
const { usePlayback } = await import('../../state/playback');
const { useUi } = await import('../../state/ui');
const { Timeline } = await import('./Timeline');
const { clockUs, holdClock, setClock } = await import('../../state/playClock');
const { zoomTimeline } = await import('./zoomControl');

// The scroller is 1240 px wide: 1200 px of lanes after the 40 px gutter. jsdom puts it at client x 0.
const width = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'clientWidth');
beforeAll(() =>
  Object.defineProperty(HTMLElement.prototype, 'clientWidth', { configurable: true, get: () => 1240 }),
);
afterAll(() => width && Object.defineProperty(HTMLElement.prototype, 'clientWidth', width));

beforeEach(() => {
  usePlayback.setState({ playheadUs: 0, playing: false });
  useUi.setState({ timelineExpanded: false, pxPerSecond: 40 });
});

const scroller = () => screen.getByTestId('ruler').closest<HTMLElement>('.overflow-auto')!;

describe('following the playhead after a zoom while playing', () => {
  beforeEach(() => {
    vi.useFakeTimers({
      toFake: ['setTimeout', 'clearTimeout', 'performance', 'requestAnimationFrame', 'cancelAnimationFrame'],
    });
    useProject.setState({ snapshot: { ...fixtureSnapshot(), project: longProject() }, pending: 0 });
  });
  afterEach(() => {
    act(() => {
      usePlayback.setState({ playing: false });
      holdClock(0);
    });
    vi.useRealTimers();
  });

  /** The lanes scrolled to `scrollPx` (as a user would), then playback running from `us`. */
  function playFrom(scrollPx: number, us: number) {
    act(() => {
      scroller().scrollLeft = scrollPx;
      scroller().dispatchEvent(new Event('scroll'));
    });
    act(() => {
      usePlayback.setState({ playing: true, playheadUs: us });
      setClock({ us, atMs: performance.now(), running: true, rate: 1 });
    });
  }
  /** Where the playing playhead is in the view (px from the lanes' left edge). */
  const playheadX = () => (clockUs() / 1e6) * useUi.getState().pxPerSecond - scroller().scrollLeft;

  it('zooming out pages when the playhead reaches the new right edge', () => {
    renderWithProviders(<Timeline />);
    // 100 s in at 40 px/s, playing at 125 s: 1000 px into the 1200 px view.
    playFrom(4000, 125_000_000);
    act(() => zoomTimeline(0.8));
    // Around the playhead: at 32 px/s it stays 1000 px in.
    expect(scroller().scrollLeft).toBeCloseTo(125 * 32 - 1000, 0);
    // It reaches the new right edge about 5.8 s later; 8 s in, the view has paged and it is still on screen.
    act(() => vi.advanceTimersByTime(8000));
    expect(playheadX()).toBeGreaterThanOrEqual(0);
    expect(playheadX()).toBeLessThanOrEqual(1200);
  });

  it('a pinch around the pointer deep in an hour keeps its anchor while the playhead stays in view', () => {
    renderWithProviders(<Timeline />);
    // 30 min in at 40 px/s, playing at 30:10 (400 px in); the pointer is at 600 px (30:15).
    playFrom(72_000, 1_810_000_000);
    fireEvent.wheel(scroller(), { ctrlKey: true, deltaY: -100 * Math.log(2), clientX: 40 + 600 });
    act(() => vi.advanceTimersToNextFrame());
    // ×2 around 30:15 at x = 600: the lanes scroll to 1815 × 80 − 600, and the playhead (about 200 px in) stays put.
    expect(useUi.getState().pxPerSecond).toBeCloseTo(80, 6);
    expect(scroller().scrollLeft).toBeCloseTo(1815 * 80 - 600, 0);
    act(() => vi.advanceTimersByTime(1000));
    expect(scroller().scrollLeft).toBeCloseTo(1815 * 80 - 600, 0);
  });
});
