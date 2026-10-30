import { act, fireEvent, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fixturePatch, fixtureProject, fixtureSnapshot } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';

vi.mock('../../bridge/commands', () => ({ bridge: { apply: vi.fn() }, mediaUrl: (p: string) => p }));
vi.mock('../../bridge/engine', () => ({
  engine: {
    preview: vi.fn().mockResolvedValue(undefined),
    previewClear: vi.fn().mockResolvedValue(undefined),
    seek: vi.fn().mockResolvedValue(undefined),
  },
}));
const { bridge } = await import('../../bridge/commands');
const { useProject } = await import('../../state/project');
const { usePlayback } = await import('../../state/playback');
const { useRange } = await import('../../state/range');
const { useSelection } = await import('../../state/selection');
const { useUi } = await import('../../state/ui');
const { Timeline } = await import('./Timeline');
const { TopBar } = await import('../../layout/TopBar');
const { useEditorShortcuts } = await import('../command-palette/useEditorShortcuts');
const { deleteRangeOps } = await import('./model/rangeOps');
const { usToPx } = await import('./scale');

const S = 1_000_000;
const frames = () => act(() => new Promise((r) => setTimeout(r, 40)));

function Editor() {
  useEditorShortcuts();
  return (
    <>
      <TopBar />
      <Timeline />
    </>
  );
}

beforeEach(() => {
  useProject.setState({ snapshot: fixtureSnapshot(), pending: 0 });
  useSelection.setState({ selectedId: null });
  useRange.setState({ range: null });
  usePlayback.setState({ playheadUs: 0, playing: false });
  useUi.setState({ pxPerSecond: 40, snapping: true });
  vi.mocked(bridge.apply).mockReset().mockResolvedValue(fixturePatch(fixtureSnapshot()));
});

describe('a time range on the timeline', () => {
  it('range_selection_deletes_with_one_deleteRanges_op', async () => {
    renderWithProviders(<Editor />);
    // A drag across empty lane space (the Sounds lane has nothing from 0:00) selects that time: 5 s to 10 s.
    const sounds = screen.getByRole('group', { name: 'Sound effects' }).querySelector<HTMLElement>('.tl-lane')!;
    fireEvent.pointerDown(sounds, { button: 0, clientX: usToPx(5 * S, 40), pointerId: 1 });
    fireEvent.pointerMove(sounds, { clientX: usToPx(7 * S, 40), pointerId: 1 });
    fireEvent.pointerMove(sounds, { clientX: usToPx(10 * S, 40), pointerId: 1 });
    fireEvent.pointerUp(sounds, { clientX: usToPx(10 * S, 40), pointerId: 1 });
    expect(useRange.getState().range).toEqual({ startUs: 5 * S, endUs: 10 * S });
    // It shows as a selection with its length, and nothing else stays selected.
    expect(screen.getByRole('note', { name: 'Selection 0:05.0 to 0:10.0' })).toHaveTextContent('5.0 s');
    expect(useSelection.getState().selectedId).toBeNull();
    // Delete takes it out of the video in one step: one deleteRanges.
    fireEvent.keyDown(window, { key: 'Delete' });
    await frames();
    expect(bridge.apply).toHaveBeenCalledTimes(1);
    expect(bridge.apply).toHaveBeenCalledWith({
      label: 'undo.delete.range',
      ops: [{ type: 'deleteRanges', ranges: [{ start: 5 * S, end: 10 * S }] }],
    });
    // Once it has gone in, the selection clears and the playhead waits where it began.
    expect(useRange.getState().range).toBeNull();
    expect(usePlayback.getState().playheadUs).toBe(5 * S);
  });

  it('a range past the clips deletes nothing, and Escape clears a range', async () => {
    const project = fixtureProject();
    expect(deleteRangeOps(project, { startUs: 40 * S, endUs: 50 * S })).toEqual([]);
    // A range reaching past the clips' end takes out only their part.
    expect(deleteRangeOps(project, { startUs: 25 * S, endUs: 50 * S })).toEqual([
      { type: 'deleteRanges', ranges: [{ start: 25 * S, end: 30 * S }] },
    ]);
    renderWithProviders(<Editor />);
    act(() => useRange.getState().select(2 * S, 4 * S));
    await userEvent.keyboard('{Escape}');
    expect(useRange.getState().range).toBeNull();
    // I and O set its start and end at the playhead.
    act(() => usePlayback.setState({ playheadUs: 3 * S }));
    await userEvent.keyboard('i');
    act(() => usePlayback.setState({ playheadUs: 8 * S }));
    await userEvent.keyboard('o');
    expect(useRange.getState().range).toEqual({ startUs: 3 * S, endUs: 8 * S });
  });

  it('empty_timeline_disables_export_with_reason', async () => {
    // Deleting everything: the engine answers with no clips and no layers left.
    vi.mocked(bridge.apply).mockImplementation(async () => {
      const s = useProject.getState().snapshot!;
      return {
        baseRevision: s.history.revision,
        history: { ...s.history, revision: s.history.revision + 1 },
        patch: {
          clips: [],
          items: [],
          main: [],
          tracks: s.project.timeline.tracks.map(({ items: _, ...track }) => ({ ...track, items: [] })),
        },
      };
    });
    renderWithProviders(<Editor />);
    const exportButton = () => screen.getByRole('button', { name: 'Export' });
    expect(exportButton()).not.toHaveAttribute('title', 'Add a video first');
    act(() => useRange.getState().select(0, 30 * S));
    fireEvent.keyDown(window, { key: 'Backspace' });
    await frames();
    expect(useProject.getState().snapshot!.project.timeline.main).toEqual([]);
    expect(exportButton()).toBeDisabled();
    expect(exportButton()).toHaveAttribute('title', 'Add a video first');
    expect(screen.getByText('Clips line up here in the order you add them.')).toBeInTheDocument();
  });
});
