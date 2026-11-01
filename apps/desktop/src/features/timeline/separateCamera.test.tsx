import { act, fireEvent, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Snapshot } from '../../bindings/Snapshot';
import { fixturePatch, fixtureSnapshot, titleItem } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';

/*
 * Separate camera: a recording clip's camera becomes a layer of its own (`detachSource` of its camera source), from the
 * clip's block menu and the commands its keys and menus run, as one undo step. Once it is a layer it is off and says
 * why; an undo brings it back.
 */

vi.mock('../../bridge/commands', () => ({
  bridge: { apply: vi.fn(), undo: vi.fn() },
  mediaUrl: (p: string) => p,
}));
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
const { useSelection } = await import('../../state/selection');
const { useUi } = await import('../../state/ui');
const { Timeline } = await import('./Timeline');
const { useCommands } = await import('../command-palette/useCommands');

const frames = () => act(() => new Promise((r) => setTimeout(r, 40)));

/** The fixture as Rust leaves it at `revision` once clip a's camera is a layer of its own, on a track of its own. */
function separated(revision: number): Snapshot {
  const snapshot = fixtureSnapshot();
  const { project } = snapshot;
  const camera = project.timeline.main[0].camera!;
  project.timeline.main[0].camera = { ...camera, mode: 'hidden', modeOverrides: { portrait: 'hidden' } };
  project.timeline.tracks.push({
    id: 't3',
    kind: 'overlay',
    name: 'Overlay 2',
    hidden: false,
    muted: false,
    locked: false,
    items: [
      {
        ...titleItem,
        id: 'a-camera',
        startUs: 200_000,
        durationUs: 19_800_000,
        layout: { ...titleItem.layout, rect: camera.bubble.rect, motion: undefined },
        attach: { clipId: 'a', source: { type: 'recording', id: 'rec' }, sourceUs: 200_000 },
        content: { type: 'video', assetId: 'camera', sourceInUs: 0, speed: 1, volume: 1, muted: true },
      },
    ],
  });
  snapshot.history = { ...snapshot.history, revision, undoLabel: 'undo.separateCamera' };
  return snapshot;
}

/** The commands as the editor has them now. */
let commands: ReturnType<typeof useCommands> = [];
function Commands() {
  commands = useCommands();
  return null;
}
const separate = () => commands.find((c) => c.id === 'separate-camera');

/** Clip `name`'s block menu, open. */
async function menuOf(name: RegExp) {
  fireEvent.contextMenu(screen.getByRole('button', { name }));
  return screen.findByRole('menuitem', { name: /^Split at playhead/ });
}

beforeEach(() => {
  useProject.setState({ snapshot: fixtureSnapshot(), pending: 0 });
  useSelection.setState({ selectedId: null });
  usePlayback.setState({ playheadUs: 5_000_000, playing: false });
  useUi.setState({ timelineExpanded: false, pxPerSecond: 40, snapping: true });
  vi.mocked(bridge.apply).mockReset();
  vi.mocked(bridge.undo).mockReset();
});

describe('Separate camera', () => {
  it("is on a recording clip's menu: one step making the camera its own layer, said in the live region", async () => {
    vi.mocked(bridge.apply).mockResolvedValue({ ...fixturePatch(separated(4)), baseRevision: 3 });
    renderWithProviders(
      <>
        <Timeline />
        <Commands />
      </>,
    );
    await menuOf(/^Recording 1/);
    await userEvent.click(screen.getByRole('menuitem', { name: /^Separate camera/ }));
    await frames();
    expect(bridge.apply).toHaveBeenCalledWith({
      label: 'undo.separateCamera',
      ops: [{ type: 'detachSource', clipId: 'a', sourceId: 'camera' }],
    });
    expect(screen.getByRole('status')).toHaveTextContent('The camera of Recording 1 is now its own layer');

    // Now a layer: off, saying why, in the menu and the commands.
    expect(separate()).toMatchObject({ enabled: false, reason: "This clip's camera is already in its own layer." });
    await menuOf(/^Recording 1/);
    const item = screen.getByRole('menuitem', { name: /^Separate camera/ });
    expect(item).toHaveAttribute('aria-disabled', 'true');
    expect(item).toHaveAccessibleDescription("This clip's camera is already in its own layer.");
    fireEvent.keyDown(item, { key: 'Escape' });
    await frames();

    // Undo puts the camera back on its clip: it can be separated again.
    vi.mocked(bridge.undo).mockResolvedValue({
      ...fixturePatch({ ...fixtureSnapshot(), history: { ...fixtureSnapshot().history, revision: 5 } }),
      baseRevision: 4,
    });
    await act(() => useProject.getState().undo());
    expect(bridge.undo).toHaveBeenCalledTimes(1);
    expect(separate()).toMatchObject({ enabled: true });
  });

  it('runs on the selected clip from the commands, and is off for a clip with no camera', async () => {
    vi.mocked(bridge.apply).mockResolvedValue({ ...fixturePatch(separated(4)), baseRevision: 3 });
    useSelection.setState({ selectedId: 'b' });
    renderWithProviders(<Commands />);
    expect(separate()).toMatchObject({ label: 'Separate camera', enabled: false });
    act(() => useSelection.setState({ selectedId: 'a' }));
    expect(separate()).toMatchObject({ enabled: true });
    act(() => separate()!.run());
    await frames();
    expect(bridge.apply).toHaveBeenCalledWith({
      label: 'undo.separateCamera',
      ops: [{ type: 'detachSource', clipId: 'a', sourceId: 'camera' }],
    });
  });

  it('stays off while a freed camera layer is still over the clip, and is on once it is moved off', () => {
    const over = separated(4);
    const layer = over.project.timeline.tracks.at(-1)!.items[0];
    layer.attach = null;
    useProject.setState({ snapshot: over });
    useSelection.setState({ selectedId: 'a' });
    renderWithProviders(<Commands />);
    expect(separate()).toMatchObject({ enabled: false, reason: "This clip's camera is already in its own layer." });
    const off = separated(5);
    Object.assign(off.project.timeline.tracks.at(-1)!.items[0], { attach: null, startUs: 30_000_000 });
    act(() => useProject.setState({ snapshot: off }));
    expect(separate()).toMatchObject({ enabled: true, reason: undefined });
  });

  it("isn't on an imported clip's menu", async () => {
    renderWithProviders(<Timeline />);
    await menuOf(/^broll\.mp4/);
    expect(screen.queryByRole('menuitem', { name: /^Separate camera/ })).toBeNull();
  });
});
