import { act, fireEvent, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Project } from '../../bindings/Project';
import type { Snapshot } from '../../bindings/Snapshot';
import { renderWithProviders } from '../../test/render';
import parity from './keys/source-mix-parity.json';

/*
 * A recording's markers on the timeline: a flag where each plays on the video, which seeks and which Delete removes.
 * Two takes from `source-mix-parity.json`: r1, with markers
 * 2:05 (m1) and 7:40 (m2) into it, played by c1 (0:00 to 5:00 of it) and c2 (6:00 to 12:00 of it, from 5:00).
 */

vi.mock('../../bridge/commands', () => ({ bridge: { apply: vi.fn() }, mediaUrl: (p: string) => p }));
vi.mock('../../bridge/engine', () => ({
  engine: {
    preview: vi.fn().mockResolvedValue(undefined),
    previewClear: vi.fn().mockResolvedValue(undefined),
    seek: vi.fn().mockResolvedValue(undefined),
  },
}));
const { bridge } = await import('../../bridge/commands');
const { engine } = await import('../../bridge/engine');
const { useProject } = await import('../../state/project');
const { usePlayback } = await import('../../state/playback');
const { useProposal, noRemovals } = await import('../../state/proposal');
const { useSelection } = await import('../../state/selection');
const { useUi } = await import('../../state/ui');
const { Timeline } = await import('./Timeline');

const twoTakes = (): Snapshot => ({
  project: structuredClone(parity.project) as unknown as Project,
  history: { revision: 3, canUndo: true, canRedo: false, undoLabel: null, redoLabel: null },
});

/** The last committed batch: (label, ops). */
const lastApply = async () => {
  await vi.waitFor(() => expect(bridge.apply).toHaveBeenCalled());
  const [tx] = vi.mocked(bridge.apply).mock.calls.at(-1)!;
  return { label: tx.label, ops: tx.ops };
};

beforeEach(() => {
  useProject.setState({ snapshot: twoTakes(), pending: 0 });
  usePlayback.setState({ playheadUs: 0, playing: false });
  useSelection.setState({ selectedId: null });
  useProposal.setState({ proposal: null, arrivedAt: 0 });
  // Zoomed out enough that both markers are in the window.
  useUi.setState({ pxPerSecond: 2, snapping: true });
  vi.mocked(engine.seek).mockClear();
  vi.mocked(bridge.apply)
    .mockReset()
    .mockImplementation(async () => {
      const s = useProject.getState().snapshot!;
      return {
        baseRevision: s.history.revision,
        history: { ...s.history, revision: s.history.revision + 1 },
        patch: { clips: [], items: [] },
      };
    });
});

describe('marker flags', () => {
  it('show where each marker plays on the video, one Tab stop for them all', () => {
    renderWithProviders(<Timeline />);
    const first = screen.getByRole('button', { name: 'Marker at 2:05' });
    // m2 is 7:40 into the take, which plays at 6:40 on the video (c2 plays 6:00 to 12:00 of it from 5:00).
    const second = screen.getByRole('button', { name: 'Marker at 6:40' });
    expect(first).toHaveAttribute('tabindex', '0');
    expect(second).toHaveAttribute('tabindex', '-1');
    // A marker in a part of the take no clip plays has no flag.
    expect(screen.getAllByRole('button', { name: /^Marker at/ })).toHaveLength(2);
  });

  it("a flag seeks there, and Delete removes it as the person's own edit", async () => {
    renderWithProviders(<Timeline />);
    const flag = screen.getByRole('button', { name: 'Marker at 2:05' });
    await userEvent.click(flag);
    await vi.waitFor(() => expect(vi.mocked(engine.seek).mock.lastCall?.[0]).toBe(125_000_000));
    expect(flag).toHaveFocus();
    await userEvent.keyboard('{Delete}');
    expect(await lastApply()).toEqual({
      label: 'undo.marker.remove',
      ops: [{ type: 'removeMarker', recordingId: 'r1', id: 'm1' }],
    });
    expect(bridge.apply).toHaveBeenCalledTimes(1);
  });

  it('the arrows move between flags, and Enter seeks', async () => {
    renderWithProviders(<Timeline />);
    const first = screen.getByRole('button', { name: 'Marker at 2:05' });
    act(() => first.focus());
    fireEvent.keyDown(first, { key: 'ArrowRight' });
    const second = screen.getByRole('button', { name: 'Marker at 6:40' });
    expect(second).toHaveFocus();
    fireEvent.keyDown(second, { key: 'Enter' });
    await vi.waitFor(() => expect(vi.mocked(engine.seek).mock.lastCall?.[0]).toBe(400_000_000));
    fireEvent.keyDown(second, { key: 'ArrowLeft' });
    expect(first).toHaveFocus();
  });

  it('a marker a waiting proposal removes is drawn as going', () => {
    act(() => useProposal.getState().show({ id: 'p', baseRevision: 3, tags: [], removals: { ...noRemovals(), markers: ['m1'] } }));
    renderWithProviders(<Timeline />);
    expect(screen.getByRole('button', { name: 'Marker at 2:05' })).toHaveAttribute('data-ghost', 'true');
    expect(screen.getByRole('button', { name: 'Marker at 6:40' })).not.toHaveAttribute('data-ghost');
  });

  it('a marker with a label says it', () => {
    const snapshot = twoTakes();
    snapshot.project.recordings[0].markers[0] = { ...snapshot.project.recordings[0].markers[0], label: 'Clutch' };
    useProject.setState({ snapshot, pending: 0 });
    renderWithProviders(<Timeline />);
    expect(screen.getByRole('button', { name: 'Clutch, marker at 2:05' })).toBeInTheDocument();
  });
});
