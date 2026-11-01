import { act, fireEvent, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Project } from '../../bindings/Project';
import type { Snapshot } from '../../bindings/Snapshot';
import { fixtureSnapshot } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';
import parity from './keys/source-mix-parity.json';

/*
 * A recording's markers on the timeline: a flag where each plays on the video, which seeks and which Delete removes;
 * M marks the moment under the playhead on its recording. Two takes from `source-mix-parity.json`: r1, with markers
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
const { useToasts } = await import('../../state/toasts');
const { useUi } = await import('../../state/ui');
const { useEditorShortcuts } = await import('../command-palette/useEditorShortcuts');
const { useCommands } = await import('../command-palette/useCommands');
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

function Keys() {
  useEditorShortcuts();
  return null;
}

function Editor() {
  return (
    <>
      <Timeline />
      <Keys />
    </>
  );
}

beforeEach(() => {
  useProject.setState({ snapshot: twoTakes(), pending: 0 });
  usePlayback.setState({ playheadUs: 0, playing: false });
  useSelection.setState({ selectedId: null });
  useProposal.setState({ proposal: null, arrivedAt: 0 });
  useToasts.setState({ toasts: [] });
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
    const removals = { ...noRemovals(), markers: ['m1'] };
    act(() => useProposal.getState().show({ id: 'p', baseRevision: 3, tags: [], removals }));
    renderWithProviders(<Timeline />);
    expect(screen.getByRole('button', { name: 'Marker at 2:05' })).toHaveAttribute('data-ghost', 'true');
    expect(screen.getByRole('button', { name: 'Marker at 6:40' })).not.toHaveAttribute('data-ghost');
  });

  it("a copied clip's flags are its own: Delete goes on to the next marker, not the copy's of the same", async () => {
    const snapshot = twoTakes();
    const { main } = snapshot.project.timeline;
    // c1 twice, then c2: m1 plays at 2:05 and again at 7:05; m2 at 11:40.
    main.splice(1, 0, { ...main[0], id: 'c1b' });
    useProject.setState({ snapshot, pending: 0 });
    renderWithProviders(<Timeline />);
    const keys = [...document.querySelectorAll<HTMLElement>('[data-marker]')].map((el) => el.dataset.marker);
    expect(keys).toHaveLength(3);
    expect(new Set(keys).size).toBe(3);
    const first = screen.getByRole('button', { name: 'Marker at 2:05' });
    await userEvent.click(first);
    await userEvent.keyboard('{Delete}');
    await lastApply();
    await vi.waitFor(() => expect(document.activeElement).toHaveAccessibleName('Marker at 11:40'));
  });

  it('a marker with a label says it', () => {
    const snapshot = twoTakes();
    snapshot.project.recordings[0].markers[0] = { ...snapshot.project.recordings[0].markers[0], label: 'Clutch' };
    useProject.setState({ snapshot, pending: 0 });
    renderWithProviders(<Timeline />);
    expect(screen.getByRole('button', { name: 'Clutch, marker at 2:05' })).toBeInTheDocument();
  });
});

describe('M', () => {
  it('marks the moment under the playhead on its recording', async () => {
    usePlayback.setState({ playheadUs: 310_000_000 });
    renderWithProviders(<Editor />);
    await userEvent.keyboard('m');
    expect(await lastApply()).toEqual({
      label: 'undo.marker.add',
      // 5:10 on the video is 6:10 of the take (c2), and m1 and m2 are taken.
      ops: [{ type: 'addMarker', recordingId: 'r1', marker: { id: 'm3', atUs: 370_000_000, kind: 'manual' } }],
    });
  });

  it('held, adds one marker, not one per key repeat', async () => {
    usePlayback.setState({ playheadUs: 310_000_000 });
    renderWithProviders(<Editor />);
    fireEvent.keyDown(window, { key: 'm', code: 'KeyM' });
    fireEvent.keyDown(window, { key: 'm', code: 'KeyM', repeat: true });
    fireEvent.keyDown(window, { key: 'm', code: 'KeyM', repeat: true });
    fireEvent.keyUp(window, { key: 'm', code: 'KeyM' });
    await lastApply();
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(bridge.apply).toHaveBeenCalledTimes(1);
  });

  it('over a clip that is no recording, says where markers go and adds none', async () => {
    useProject.setState({ snapshot: fixtureSnapshot(), pending: 0 });
    // 25 s is in b, an imported clip.
    usePlayback.setState({ playheadUs: 25_000_000 });
    renderWithProviders(<Editor />);
    await userEvent.keyboard('m');
    await vi.waitFor(() =>
      expect(useToasts.getState().toasts.map((toast) => toast.text)).toContain(
        'Markers go on a recording. Move the playhead over one.',
      ),
    );
    expect(bridge.apply).not.toHaveBeenCalled();
  });

  it('is the Add marker command, off without a recording in the video', () => {
    let commands: ReturnType<typeof useCommands> = [];
    function Probe() {
      commands = useCommands();
      return null;
    }
    const { unmount } = renderWithProviders(<Probe />);
    expect(commands.find((c) => c.id === 'marker-add')).toMatchObject({
      label: 'Add marker',
      shortcut: 'M',
      enabled: true,
    });
    unmount();
    const snapshot = fixtureSnapshot();
    snapshot.project.timeline.main = snapshot.project.timeline.main.filter((c) => c.source.type === 'asset');
    useProject.setState({ snapshot, pending: 0 });
    renderWithProviders(<Probe />);
    expect(commands.find((c) => c.id === 'marker-add')?.enabled).toBe(false);
  });
});
