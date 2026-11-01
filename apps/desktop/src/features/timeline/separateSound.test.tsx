import { act, fireEvent, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Snapshot } from '../../bindings/Snapshot';
import { fixturePatch, fixtureSnapshot, titleItem } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';

/*
 * Separate sound: an imported clip's own sound goes on a layer of its own under it (`detachSound`), once, drawn in the
 * clip's blue as a take's separated sound is.
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
const { useEditorShortcuts } = await import('../command-palette/useEditorShortcuts');

const S = 1_000_000;
const frames = () => act(() => new Promise((r) => setTimeout(r, 40)));

let commands: ReturnType<typeof useCommands> = [];
function Commands() {
  commands = useCommands();
  useEditorShortcuts();
  return null;
}
const command = (id: string) => commands.find((c) => c.id === id);

/** Clip `name`'s block menu, open. */
async function menuOf(name: RegExp) {
  fireEvent.contextMenu(screen.getByRole('button', { name }));
  return screen.findByRole('menuitem', { name: /^Split at playhead/ });
}

/** The fixture once clip b's sound is on a layer of its own. */
function separated(revision: number): Snapshot {
  const snapshot = fixtureSnapshot();
  snapshot.project.timeline.main[1].muted = true;
  snapshot.project.timeline.tracks.push({
    id: 't9',
    kind: 'audio',
    name: 'Audio 2',
    hidden: false,
    muted: false,
    locked: false,
    items: [
      {
        ...titleItem,
        id: 'b-sound',
        startUs: 20 * S,
        durationUs: 10 * S,
        layout: { ...titleItem.layout, motion: undefined },
        attach: { clipId: 'b', source: { type: 'asset', id: 'broll' }, sourceUs: 0 },
        content: { type: 'audio', assetId: 'broll', sourceInUs: 0, volume: 1, fadeInUs: 0, fadeOutUs: 0, effect: null },
      },
    ],
  });
  snapshot.history = { ...snapshot.history, revision, undoLabel: 'undo.sound.separate' };
  return snapshot;
}

beforeEach(() => {
  useProject.setState({ snapshot: fixtureSnapshot(), pending: 0 });
  useSelection.setState({ selectedId: null });
  usePlayback.setState({ playheadUs: 25 * S, playing: false });
  useUi.setState({ timelineExpanded: false, pxPerSecond: 40, snapping: true });
  vi.mocked(bridge.apply).mockReset().mockResolvedValue({ ...fixturePatch(separated(4)), baseRevision: 3 });
  vi.mocked(bridge.undo).mockReset();
});

describe('Separate sound', () => {
  it("is on an imported clip's menu: one step putting its sound on a layer of its own", async () => {
    renderWithProviders(
      <>
        <Timeline />
        <Commands />
      </>,
    );
    await menuOf(/^broll\.mp4/);
    await userEvent.click(screen.getByRole('menuitem', { name: /^Separate sound/ }));
    await frames();
    expect(bridge.apply).toHaveBeenCalledWith({
      label: 'undo.sound.separate',
      ops: [{ type: 'detachSound', clipId: 'b' }],
    });
    expect(screen.getByRole('status')).toHaveTextContent('The sound of broll.mp4 is now its own layer');
    // Once separated: off, saying why.
    act(() => useSelection.setState({ selectedId: 'b' }));
    expect(command('separate-sound')).toMatchObject({
      enabled: false,
      reason: "This clip's sound is already in its own layer.",
    });
  });

  it('is not listed for a recording clip, and is off for a sped-up clip, saying why', async () => {
    renderWithProviders(
      <>
        <Timeline />
        <Commands />
      </>,
    );
    await menuOf(/^Recording 1/);
    expect(screen.queryByRole('menuitem', { name: /^Separate sound/ })).toBeNull();
    fireEvent.keyDown(screen.getByRole('menuitem', { name: /^Split at playhead/ }), { key: 'Escape' });
    const fast = fixtureSnapshot();
    fast.project.timeline.main[1].speed = 2;
    act(() => {
      useProject.setState({ snapshot: fast });
      useSelection.setState({ selectedId: 'b' });
    });
    expect(command('separate-sound')).toMatchObject({
      enabled: false,
      reason: 'Set the clip back to normal speed before separating its sound.',
    });
  });

  it("draws the separated sound in the clip's blue, as a take's own sound", () => {
    useProject.setState({ snapshot: separated(4) });
    renderWithProviders(<Timeline />);
    const block = document.querySelector('[data-block-id="b-sound"]');
    expect(block).toHaveClass('is-recorded');
  });
});
