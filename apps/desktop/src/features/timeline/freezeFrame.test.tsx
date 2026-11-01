import { act, fireEvent, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Clip } from '../../bindings/Clip';
import type { Snapshot } from '../../bindings/Snapshot';
import { fixturePatch, fixtureSnapshot } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';

/*
 * Freeze frame (⌥F, a clip's menu, the palette) holds the frame at the playhead for 2 s (`freezeFrame`): the clip there
 * is split around it. Its block draws that frame, and its card has no speed or sound.
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
const { ClipInspector } = await import('../inspector/ClipInspector');

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

/** The fixture with a 2 s freeze of the b-roll's first frame between "a" and "b", as Rust leaves it. */
function frozen(revision = 3): Snapshot {
  const snapshot = fixtureSnapshot();
  const b = snapshot.project.timeline.main[1];
  const freeze: Clip = { ...b, id: 'b-freeze', freezeAtUs: 0, sourceInUs: 0, sourceOutUs: 2 * S, muted: true };
  snapshot.project.timeline.main.splice(1, 0, freeze);
  snapshot.history = { ...snapshot.history, revision };
  return snapshot;
}

beforeEach(() => {
  useProject.setState({ snapshot: fixtureSnapshot(), pending: 0 });
  useSelection.setState({ selectedId: null });
  usePlayback.setState({ playheadUs: 25 * S, playing: false });
  useUi.setState({ timelineExpanded: false, pxPerSecond: 40, snapping: true });
  vi.mocked(bridge.apply).mockReset().mockResolvedValue({ ...fixturePatch(frozen(4)), baseRevision: 3 });
  vi.mocked(bridge.undo).mockReset();
});

describe('Freeze frame', () => {
  it('⌥F holds the frame at the playhead for 2 seconds, as one step said in the live region', async () => {
    renderWithProviders(
      <>
        <Timeline />
        <Commands />
      </>,
    );
    expect(command('freeze-frame')).toMatchObject({ label: 'Freeze frame', shortcut: '⌥F', enabled: true });
    // ⌥ turns F into "ƒ" on a Mac: the key's code is read.
    fireEvent.keyDown(document.body, { key: 'ƒ', code: 'KeyF', altKey: true });
    await frames();
    expect(bridge.apply).toHaveBeenCalledWith({
      label: 'undo.freeze',
      ops: [{ type: 'freezeFrame', atUs: 25 * S, durationUs: 2 * S }],
    });
    expect(screen.getByRole('status')).toHaveTextContent('Froze broll.mp4 at 0:25 for 2 s');
  });

  it("is on a clip's menu, and acts on that clip alone: at its end, its last frame", async () => {
    usePlayback.setState({ playheadUs: 20 * S });
    renderWithProviders(
      <>
        <Timeline />
        <Commands />
      </>,
    );
    await menuOf(/^Recording 1/);
    await userEvent.click(screen.getByRole('menuitem', { name: /^Freeze frame/ }));
    await frames();
    // 20 s is "b"'s start too: from "a"'s menu it holds "a"'s last frame.
    expect(bridge.apply).toHaveBeenCalledWith({
      label: 'undo.freeze',
      ops: [{ type: 'freezeFrame', atUs: 20 * S - 1, durationUs: 2 * S }],
    });
  });

  it('says why it is off with nothing to freeze', () => {
    const empty = fixtureSnapshot();
    empty.project.timeline.main = [];
    useProject.setState({ snapshot: empty });
    renderWithProviders(<Commands />);
    expect(command('freeze-frame')).toMatchObject({ enabled: false });
  });

  it('draws as its frame, named as a freeze, and its card has no speed or sound', () => {
    useProject.setState({ snapshot: frozen() });
    renderWithProviders(
      <>
        <Timeline />
        <ClipInspector clip={frozen().project.timeline.main[1]} format="landscape" />
      </>,
    );
    expect(screen.getByRole('button', { name: /^Freeze frame of broll\.mp4, clip 2 of 3, 0:02/ })).toBeInTheDocument();
    expect(screen.queryByText('Speed')).toBeNull();
    expect(screen.queryByText('Volume')).toBeNull();
    expect(screen.getByText('Holds the frame at 0:00. Trim it to hold it longer or shorter.')).toBeInTheDocument();
  });
});

