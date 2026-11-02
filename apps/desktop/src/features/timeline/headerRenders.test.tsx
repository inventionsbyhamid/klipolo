import { act } from '@testing-library/react';
import { afterAll, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import { fixtureSnapshot } from '../../test/fixtures';
import { longProjectWithSound } from '../../test/longProject';
import { renderWithProviders } from '../../test/render';

/** The lanes' header buttons and icon tooltips, counted as they draw. */
const drawn = vi.hoisted(() => ({ buttons: 0, tips: 0 }));
vi.mock('../../design/IconButton', async (original) => {
  const real = await original<typeof import('../../design/IconButton')>();
  return {
    ...real,
    IconButton: (props: Parameters<typeof real.IconButton>[0]) => {
      drawn.buttons += 1;
      return <real.IconButton {...props} />;
    },
  };
});
vi.mock('../../design/Tooltip', async (original) => {
  const real = await original<typeof import('../../design/Tooltip')>();
  return {
    ...real,
    Tooltip: (props: Parameters<typeof real.Tooltip>[0]) => {
      drawn.tips += 1;
      return <real.Tooltip {...props} />;
    },
  };
});
vi.mock('../../bridge/commands', () => ({ bridge: { apply: vi.fn() }, mediaUrl: (p: string) => p }));
const { useProject } = await import('../../state/project');
const { useUi } = await import('../../state/ui');
const { useSelection } = await import('../../state/selection');
const { usePlayback } = await import('../../state/playback');
const { Timeline } = await import('./Timeline');

const width = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'clientWidth');
beforeAll(() =>
  Object.defineProperty(HTMLElement.prototype, 'clientWidth', { configurable: true, get: () => 1240 }),
);
afterAll(() => width && Object.defineProperty(HTMLElement.prototype, 'clientWidth', width));

beforeEach(() => {
  useProject.setState({ snapshot: { ...fixtureSnapshot(), project: longProjectWithSound() }, pending: 0 });
  useUi.setState({ timelineExpanded: false, pxPerSecond: 40 });
  useSelection.setState({ selectedId: null });
  usePlayback.setState({ playheadUs: 0, playing: false });
});

describe("the lanes' headers", () => {
  it('draw again for none of a zoom, a selection or an edit of a block', () => {
    renderWithProviders(<Timeline />);
    expect(drawn.buttons).toBeGreaterThan(0);
    drawn.buttons = 0;
    drawn.tips = 0;
    act(() => useUi.setState({ pxPerSecond: 50 }));
    act(() => useSelection.getState().select('c3'));
    act(() => {
      const s = useProject.getState().snapshot!;
      const main = s.project.timeline.main.map((c, i) => (i === 3 ? { ...c, volume: 0.5 } : c));
      useProject.setState({ snapshot: { ...s, project: { ...s.project, timeline: { ...s.project.timeline, main } } } });
    });
    expect(drawn.buttons).toBe(0);
    expect(drawn.tips).toBe(0);
  });
});
