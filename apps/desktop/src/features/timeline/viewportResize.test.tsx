import { act } from '@testing-library/react';
import { Profiler } from 'react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { fixtureSnapshot } from '../../test/fixtures';
import { stubSize } from '../../test/layoutBox';
import { renderWithProviders } from '../../test/render';
import { captureResizeObservers } from '../../test/resizeObservers';

vi.mock('../../bridge/commands', () => ({ bridge: { apply: vi.fn() }, mediaUrl: (p: string) => p }));
vi.mock('../../bridge/engine', () => ({ engine: { seek: vi.fn(() => Promise.resolve()) } }));
const { useProject } = await import('../../state/project');
const { usePlayback } = await import('../../state/playback');
const { useUi } = await import('../../state/ui');
const { Timeline } = await import('./Timeline');

/**
 * The lanes' scroller, read live. At 1240 px it shows 1200 px of lanes after the 40 px gutter: 30 s at 40 px/s, so
 * the lanes drawn (the view and one view each side) reach 60 s.
 */
const view = { width: 1240, height: 200 };
let observers: ReturnType<typeof captureResizeObservers>;
let unstub: () => void;
beforeEach(() => {
  useProject.setState({ snapshot: fixtureSnapshot(), pending: 0 });
  useUi.setState({ timelineExpanded: false, pxPerSecond: 40 });
  usePlayback.setState({ playheadUs: 0, playing: false });
  Object.assign(view, { width: 1240, height: 200 });
  observers = captureResizeObservers();
  unstub = stubSize('[data-timeline] .overflow-auto', view);
});
afterEach(() => {
  unstub();
  observers.restore();
  vi.useRealTimers();
});

/** Renders the timeline and returns how many commits it has made so far. */
function renderCounted(): () => number {
  let renders = 0;
  renderWithProviders(
    <Profiler id="timeline" onRender={() => (renders += 1)}>
      <Timeline />
    </Profiler>,
  );
  return () => renders;
}
const resize = (box: Partial<typeof view>) => {
  Object.assign(view, box);
  act(() => observers.fire());
};
const holdTimers = () => vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });

describe('the timeline while the window or its bar resizes it', () => {
  it('a height change (the timeline bar’s drag) renders nothing', () => {
    const renders = renderCounted();
    const start = renders();
    for (const height of [220, 240, 260, 280]) resize({ height });
    expect(renders()).toBe(start);
  });

  it('a width change the drawn lanes still cover renders once, 120 ms after the last', () => {
    holdTimers();
    const renders = renderCounted();
    const start = renders();
    for (const width of [1300, 1250, 1400, 1500]) resize({ width });
    act(() => vi.advanceTimersByTime(119));
    expect(renders()).toBe(start);
    act(() => vi.advanceTimersByTime(1));
    expect(renders()).toBe(start + 1);
  });

  it('a window widened past the drawn lanes draws them at once', () => {
    holdTimers();
    const renders = renderCounted();
    const start = renders();
    resize({ width: 1500 });
    // 2472 px of lanes is 61.8 s, past the 60 s drawn.
    resize({ width: 2600 });
    expect(renders()).toBe(start + 1);
    act(() => vi.advanceTimersByTime(500));
    expect(renders()).toBe(start + 1);
  });
});
