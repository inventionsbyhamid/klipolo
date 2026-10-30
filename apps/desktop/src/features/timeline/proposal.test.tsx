import { act, within } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fixtureSnapshot } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';

vi.mock('../../bridge/commands', () => ({ bridge: { apply: vi.fn() }, mediaUrl: (p: string) => p }));
vi.mock('../../bridge/engine', () => ({
  engine: {
    preview: vi.fn().mockResolvedValue(undefined),
    previewClear: vi.fn().mockResolvedValue(undefined),
    seek: vi.fn().mockResolvedValue(undefined),
  },
}));
const { useProject } = await import('../../state/project');
const { usePlayback } = await import('../../state/playback');
const { useProposal, noRemovals } = await import('../../state/proposal');
const { useUi } = await import('../../state/ui');
const { Timeline } = await import('./Timeline');
const { usToPx } = await import('./scale');

const S = 1_000_000;

beforeEach(() => {
  useProject.setState({ snapshot: fixtureSnapshot(), pending: 0 });
  useProposal.setState({ proposal: null, arrivedAt: 0 });
  usePlayback.setState({ playheadUs: 0, playing: false });
  useUi.setState({ pxPerSecond: 40 });
});

describe('a proposal waiting on the timeline', () => {
  it('proposal_ranges_reach_the_timeline', () => {
    renderWithProviders(<Timeline />);
    expect(document.querySelectorAll('[data-cut]')).toHaveLength(0);
    // Chat proposes two cuts: an "um" at 2 s and a pause at 12 s.
    act(() =>
      useProposal.getState().show({
        id: 'p1',
        baseRevision: 3,
        removals: {
          ...noRemovals(),
          ranges: [
            { start: 2 * S, end: 2.5 * S },
            { start: 12 * S, end: 13.5 * S },
          ],
        },
        tags: [
          { startUs: 2 * S, endUs: 2.5 * S, label: 'um' },
          { startUs: 12 * S, endUs: 13.5 * S, label: 'pause' },
        ],
      }),
    );
    const cuts = [...document.querySelectorAll<HTMLElement>('[data-cut]')];
    expect(cuts).toHaveLength(2);
    // Each at its time over every lane, at least 4 px wide, with its tag; drawing in as the proposal arrives.
    expect(cuts[0]).toHaveStyle({ left: `${usToPx(2 * S, 40)}px`, width: `${usToPx(0.5 * S, 40)}px` });
    expect(cuts.map((c) => c.textContent)).toEqual(['um', 'pause']);
    expect(cuts[0]).toHaveClass('is-new');
    expect(within(cuts[1]).getByText('pause')).toHaveClass('tl-cut-tag');
    // The overview strip marks each cut too.
    expect(document.querySelectorAll('[data-overview-cut]').length).toBeGreaterThan(0);
    act(() => useProposal.getState().show(null));
    expect(document.querySelectorAll('[data-cut]')).toHaveLength(0);
  });

  it('tags never overlap: one that would run into the last shown is left off, however long its words', () => {
    renderWithProviders(<Timeline />);
    // At 40 px/s: a long reason at 2 s (about 160 px of tag), an "um" 2 s later (80 px on) that its tag would cover, and
    // a "pause" far enough along to show.
    const range = (s: number) => ({ start: s * S, end: (s + 0.3) * S });
    act(() =>
      useProposal.getState().show({
        id: 'p2',
        baseRevision: 3,
        removals: { ...noRemovals(), ranges: [range(2), range(4), range(10)] },
        tags: [
          { startUs: 2 * S, endUs: 2.3 * S, label: 'a long pause while thinking' },
          { startUs: 4 * S, endUs: 4.3 * S, label: 'um' },
          { startUs: 10 * S, endUs: 10.3 * S, label: 'pause' },
        ],
      }),
    );
    const tags = [...document.querySelectorAll<HTMLElement>('[data-cut]')].map((c) => c.textContent);
    expect(tags).toEqual(['a long pause while thinking', '', 'pause']);
    act(() => useProposal.getState().show(null));
  });
});
