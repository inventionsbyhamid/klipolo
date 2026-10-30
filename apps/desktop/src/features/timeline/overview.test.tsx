import { act } from '@testing-library/react';
import { afterAll, beforeAll, beforeEach, expect, it, vi } from 'vitest';
import { fixtureSnapshot } from '../../test/fixtures';
import { longProject } from '../../test/longProject';
import { renderWithProviders } from '../../test/render';

vi.mock('../../bridge/commands', () => ({ bridge: { apply: vi.fn() }, mediaUrl: (p: string) => p }));
const { useProject } = await import('../../state/project');
const { usePlayback } = await import('../../state/playback');
const { useUi } = await import('../../state/ui');
const { Timeline } = await import('./Timeline');

// The scroller is 1240 px wide: 1200 px of lanes after the 40 px gutter.
const width = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'clientWidth');
beforeAll(() =>
  Object.defineProperty(HTMLElement.prototype, 'clientWidth', { configurable: true, get: () => 1240 }),
);
afterAll(() => width && Object.defineProperty(HTMLElement.prototype, 'clientWidth', width));

beforeEach(() => {
  useProject.setState({ snapshot: { ...fixtureSnapshot(), project: longProject() }, pending: 0 });
  usePlayback.setState({ playheadUs: 0, playing: false });
  useUi.setState({ pxPerSecond: 40 });
});

it('the overview strip outlines the part of the hour the lanes show, from the first render, and follows the scroll', () => {
  renderWithProviders(<Timeline />);
  const strip = document.querySelector<HTMLElement>('[data-overview]')!;
  const outline = strip.querySelector<HTMLElement>('.ov-view')!;
  // 1200 px of an hour at 40 px/s (144,000 px): a 30 s window at the start.
  expect(strip.dataset.fits).toBe('false');
  expect(parseFloat(outline.style.width)).toBeCloseTo((1200 / 144_000) * 100, 3);
  expect(outline.style.left).toBe('0%');
  const scroller = document.querySelector<HTMLElement>('[data-testid="ruler"]')!.closest<HTMLElement>('.tl-scroll')!;
  act(() => {
    scroller.scrollLeft = 72_000;
    scroller.dispatchEvent(new Event('scroll'));
  });
  expect(outline.style.left).toBe('50%');
});
