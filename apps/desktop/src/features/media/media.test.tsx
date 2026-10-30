import { renderHook } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Asset } from '../../bindings/Asset';
import { fixturePatch, fixtureSnapshot } from '../../test/fixtures';

vi.mock('../../bridge/commands', () => ({
  bridge: { apply: vi.fn(), importMedia: vi.fn() },
  mediaUrl: (p: string) => p,
}));
const { bridge } = await import('../../bridge/commands');
const { useProject } = await import('../../state/project');
const { usePlayback } = await import('../../state/playback');
const { useSelection } = await import('../../state/selection');
const { useToasts } = await import('../../state/toasts');
const { importFiles } = await import('./useImportMedia');
const { useFinderDrop } = await import('./useFinderDrop');

beforeEach(() => {
  useProject.setState({ snapshot: fixtureSnapshot(), pending: 0 });
  useSelection.setState({ selectedId: null });
  usePlayback.setState({ playheadUs: 2_000_000 });
  useToasts.setState({ toasts: [] });
  vi.mocked(bridge.apply).mockReset().mockResolvedValue(fixturePatch(fixtureSnapshot()));
  vi.mocked(bridge.importMedia).mockReset();
});

describe('importing media', () => {
  it('files dropped on the window import as one undo step, videos after the last clip in the order given', async () => {
    const broll = fixtureSnapshot().project.assets[5];
    const a: Asset = { ...broll, id: 'n1', path: '/n1.mp4', name: 'n1.mp4' };
    const b: Asset = { ...broll, id: 'n2', path: '/n2.mp4', name: 'n2.mp4' };
    vi.mocked(bridge.importMedia).mockResolvedValue({ assets: [a, b], skipped: [] });
    await importFiles(['/n1.mp4', '/n2.mp4']);
    expect(bridge.importMedia).toHaveBeenCalledWith(['/n1.mp4', '/n2.mp4']);
    await vi.waitFor(() => expect(bridge.apply).toHaveBeenCalledTimes(1));
    const tx = vi.mocked(bridge.apply).mock.calls[0][0];
    expect(tx.label).toBe('media.import');
    expect(tx.ops).toMatchObject([
      { type: 'addAssets', assets: [{ id: 'n1' }, { id: 'n2' }] },
      // `index: null` appends: after the last clip, in the order given.
      { type: 'insertClip', index: null, clip: { source: { type: 'asset', id: 'n1' } } },
      { type: 'insertClip', index: null, clip: { source: { type: 'asset', id: 'n2' } } },
    ]);
  });

  it('names the files it skipped with why: a note when something came in, an error when nothing did', async () => {
    const broll = fixtureSnapshot().project.assets[5];
    vi.mocked(bridge.importMedia).mockResolvedValueOnce({
      assets: [{ ...broll, id: 'n1', path: '/n1.mp4', name: 'n1.mp4' }],
      skipped: [
        { name: 'notes.txt', reason: "This file type isn't supported." },
        { name: 'broken.mov', reason: "This file couldn't be read." },
      ],
    });
    await importFiles(['/n1.mp4', '/notes.txt', '/broken.mov']);
    expect(useToasts.getState().toasts.map((t) => [t.kind, t.text])).toEqual([
      [
        'info',
        "Skipped 2 files. notes.txt: this file type isn't supported. broken.mov: this file couldn't be read.",
      ],
    ]);
    useToasts.setState({ toasts: [] });
    vi.mocked(bridge.importMedia).mockResolvedValueOnce({
      assets: [],
      skipped: [{ name: 'notes.txt', reason: "This file type isn't supported." }],
    });
    await importFiles(['/notes.txt']);
    expect(useToasts.getState().toasts.map((t) => [t.kind, t.text])).toEqual([
      ['error', "Skipped notes.txt: this file type isn't supported."],
    ]);
  });

  it('nothing is placed into a project opened while the files were read', async () => {
    let read!: (value: { assets: Asset[]; skipped: [] }) => void;
    vi.mocked(bridge.importMedia).mockReturnValue(new Promise((resolve) => (read = resolve)));
    const importing = importFiles(['/n1.mp4']);
    useProject.setState({
      snapshot: { ...fixtureSnapshot(), project: { ...fixtureSnapshot().project, id: 'another' } },
    });
    read({ assets: [{ ...fixtureSnapshot().project.assets[5], id: 'n1' }], skipped: [] });
    await importing;
    expect(bridge.apply).not.toHaveBeenCalled();
  });

  it('the Finder listener mounts without Tauri', () => {
    const { unmount } = renderHook(() => useFinderDrop());
    unmount();
  });
});
