import { act, fireEvent, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
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
const { bridge } = await import('../../bridge/commands');
const { useProject } = await import('../../state/project');
const { usePlayback } = await import('../../state/playback');
const { useSelection } = await import('../../state/selection');
const { useToasts } = await import('../../state/toasts');
const { useUi } = await import('../../state/ui');
const { Timeline } = await import('./Timeline');
const { useEditorShortcuts } = await import('../command-palette/useEditorShortcuts');

const frames = () => act(() => new Promise((r) => setTimeout(r, 40)));

function ShortcutsHarness() {
  useEditorShortcuts();
  return null;
}

function renderTimeline() {
  renderWithProviders(
    <>
      <Timeline />
      <ShortcutsHarness />
    </>,
  );
}

beforeEach(() => {
  useProject.setState({ snapshot: fixtureSnapshot(), pending: 0 });
  useSelection.setState({ selectedId: null });
  useToasts.setState({ toasts: [] });
  usePlayback.setState({ playheadUs: 25_000_000, playing: false });
  useUi.setState({ timelineExpanded: false, pxPerSecond: 40, snapping: true });
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

describe("a block's menu", () => {
  it('Shift+F10 opens the block menu, and each edit is announced', async () => {
    renderTimeline();
    const title = screen.getByRole('button', { name: /Set up billing/ });
    act(() => title.focus());
    fireEvent.keyDown(title, { key: 'F10', shiftKey: true });
    expect(await screen.findByRole('menuitem', { name: /Split/ })).toBeInTheDocument();
    await userEvent.click(screen.getByRole('menuitem', { name: /Free from clip/ }));
    expect(bridge.apply).toHaveBeenCalledWith({
      label: 'undo.attach.free',
      ops: [{ type: 'setAttach', id: 'title', attached: false }],
    });
    expect(screen.getByRole('status')).toHaveTextContent('Set up billing no longer moves with Recording 1');
  });

  it('Delete from a menu the keyboard opened moves focus and the selection on, as ⌫ does', async () => {
    // The engine takes a out: b is the only clip left.
    vi.mocked(bridge.apply).mockImplementation(async () => {
      const s = useProject.getState().snapshot!;
      return {
        baseRevision: s.history.revision,
        history: { ...s.history, revision: s.history.revision + 1 },
        patch: { main: ['b'], clips: [], items: [] },
      };
    });
    renderTimeline();
    const a = screen.getByRole('button', { name: /^Recording 1/ });
    act(() => a.focus());
    fireEvent.keyDown(a, { key: 'F10', shiftKey: true });
    await userEvent.click(await screen.findByRole('menuitem', { name: /Delete clip/ }));
    await frames();
    expect(bridge.apply).toHaveBeenCalledWith({
      label: 'undo.delete.clip',
      ops: [{ type: 'removeClip', id: 'a' }],
    });
    expect(useSelection.getState().selectedId).toBe('b');
    expect(document.activeElement).toBe(screen.getByRole('button', { name: /^broll\.mp4/ }));
  });

  it("Split at playhead splits the menu's own clip, or says why it can't", async () => {
    // The playhead is in Recording 1 (0 to 20 s); the menu is broll.mp4's (20 to 30 s).
    usePlayback.setState({ playheadUs: 5_000_000 });
    renderTimeline();
    const broll = screen.getByRole('button', { name: /^broll\.mp4/ });
    fireEvent.contextMenu(broll);
    await userEvent.click(await screen.findByRole('menuitem', { name: /Split at playhead/ }));
    await frames();
    expect(bridge.apply).not.toHaveBeenCalled();
    expect(useToasts.getState().toasts.at(-1)?.text).toBe("The playhead isn't inside the selected clip.");
    // With the playhead inside it, that clip splits.
    act(() => usePlayback.setState({ playheadUs: 25_000_000 }));
    fireEvent.contextMenu(broll);
    await userEvent.click(await screen.findByRole('menuitem', { name: /Split at playhead/ }));
    await frames();
    expect(bridge.apply).toHaveBeenCalledWith({
      label: 'undo.split.clip',
      ops: [{ type: 'split', atUs: 25_000_000 }],
    });
  });
});
