import { act, fireEvent, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Item } from '../../bindings/Item';
import type { Snapshot } from '../../bindings/Snapshot';
import { fixtureSnapshot, titleItem } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';

/*
 * A layer's block menu: Mute for a video layer with sound as for a sound layer, the same command as the palette's.
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
const { useProject } = await import('../../state/project');
const { usePlayback } = await import('../../state/playback');
const { useSelection } = await import('../../state/selection');
const { useUi } = await import('../../state/ui');
const { Timeline } = await import('./Timeline');

const frames = () => act(() => new Promise((r) => setTimeout(r, 40)));

/** A video layer of `pip.mp4` (with sound unless `silent`) at 1 s, over the title, on the overlay track. */
function withVideo({ muted = false, silent = false } = {}): Snapshot {
  const snapshot = fixtureSnapshot();
  const { project } = snapshot;
  const broll = project.assets.find((a) => a.id === 'broll')!;
  project.assets.push({ ...broll, id: 'pip', name: 'pip.mp4', path: '/media/pip', hasAudio: !silent });
  const layer: Item = {
    ...titleItem,
    id: 'v1',
    startUs: 5_000_000,
    attach: null,
    content: { type: 'video', assetId: 'pip', sourceInUs: 0, speed: 1, volume: 0.8, muted },
  };
  project.timeline.tracks[0].items.push(layer);
  return snapshot;
}

async function openMenu(name: RegExp) {
  fireEvent.contextMenu(screen.getByRole('button', { name }));
  return screen.findAllByRole('menuitem');
}

const names = (items: HTMLElement[]) => items.map((item) => item.textContent);

beforeEach(() => {
  useSelection.setState({ selectedId: null });
  usePlayback.setState({ playheadUs: 6_000_000, playing: false });
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

describe("a layer's block menu", () => {
  it('a video layer with sound has Mute video, which sets its own switch and keeps its volume', async () => {
    useProject.setState({ snapshot: withVideo(), pending: 0 });
    renderWithProviders(<Timeline />);
    await openMenu(/^Video layer, pip\.mp4/);
    await userEvent.click(screen.getByRole('menuitem', { name: 'Mute video' }));
    await frames();
    expect(bridge.apply).toHaveBeenCalledWith({
      label: 'volume.mute.undo',
      params: { noun: 'video' },
      ops: [
        {
          type: 'updateItem',
          id: 'v1',
          patch: { content: expect.objectContaining({ type: 'video', muted: true, volume: 0.8 }) },
        },
      ],
    });
    expect(screen.getByRole('status')).toHaveTextContent('Muted pip.mp4');
  });

  it('a muted video layer says Unmute video; one without sound has no Mute', async () => {
    useProject.setState({ snapshot: withVideo({ muted: true }), pending: 0 });
    const { unmount } = renderWithProviders(<Timeline />);
    await openMenu(/^Video layer, pip\.mp4/);
    expect(screen.getByRole('menuitem', { name: 'Unmute video' })).toBeInTheDocument();
    fireEvent.keyDown(document.activeElement!, { key: 'Escape' });
    unmount();
    useProject.setState({ snapshot: withVideo({ silent: true }), pending: 0 });
    renderWithProviders(<Timeline />);
    const items = await openMenu(/^Video layer, pip\.mp4/);
    expect(names(items).some((name) => /mute/i.test(name ?? ''))).toBe(false);
  });
});
