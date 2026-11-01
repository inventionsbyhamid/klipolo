import { act, fireEvent, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Asset } from '../../bindings/Asset';
import type { ReversedCopy } from '../../bindings/ReversedCopy';
import type { Snapshot } from '../../bindings/Snapshot';
import { fixturePatch, fixtureSnapshot } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';

/*
 * Reverse: an imported clip plays backward. Its footage is written backward first (the `reverser` bridge), its progress
 * in the top bar's pill with a stop; then the copy and the turn go in as one undo step. A reversed clip plays forward
 * again at once.
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
vi.mock('../../bridge/reverse', () => ({ reverser: { make: vi.fn(), stop: vi.fn().mockResolvedValue(undefined) } }));
const { bridge } = await import('../../bridge/commands');
const { reverser } = await import('../../bridge/reverse');
const { useProject } = await import('../../state/project');
const { usePlayback } = await import('../../state/playback');
const { useSelection } = await import('../../state/selection');
const { useUi } = await import('../../state/ui');
const { useReversing } = await import('../../state/reversing');
const { Timeline } = await import('./Timeline');
const { ReversingPill } = await import('../media-jobs/ReversingPill');
const { useCommands } = await import('../command-palette/useCommands');

const S = 1_000_000;
const frames = () => act(() => new Promise((r) => setTimeout(r, 40)));

let commands: ReturnType<typeof useCommands> = [];
function Commands() {
  commands = useCommands();
  return null;
}
const reverse = () => commands.find((c) => c.id === 'reverse');

/** The b-roll written backward, as `clip_reverse` answers it. */
const copyAsset: Asset = {
  id: 'broll-rev',
  kind: 'video',
  path: '/media/broll reversed.mov',
  name: 'broll reversed.mov',
  durationUs: 10 * S,
  width: 1920,
  height: 1080,
  fps: 30,
  hasAudio: true,
};

/** The fixture once clip b plays backward from the copy. */
function reversed(revision = 3): Snapshot {
  const snapshot = fixtureSnapshot();
  snapshot.project.assets.push(copyAsset);
  const b = snapshot.project.timeline.main[1];
  snapshot.project.timeline.main[1] = {
    ...b,
    source: { type: 'asset', id: 'broll-rev' },
    reversed: { of: 'broll', fromUs: 0, toUs: 10 * S },
  };
  snapshot.history = { ...snapshot.history, revision };
  return snapshot;
}

/** Clip `name`'s block menu, open. */
async function menuOf(name: RegExp) {
  fireEvent.contextMenu(screen.getByRole('button', { name }));
  return screen.findByRole('menuitem', { name: /^Split at playhead/ });
}

beforeEach(() => {
  useProject.setState({ snapshot: fixtureSnapshot(), pending: 0 });
  useSelection.setState({ selectedId: null });
  usePlayback.setState({ playheadUs: 25 * S, playing: false });
  useUi.setState({ timelineExpanded: false, pxPerSecond: 40, snapping: true });
  useReversing.setState({ jobs: [] });
  vi.mocked(bridge.apply).mockReset().mockResolvedValue({ ...fixturePatch(reversed(4)), baseRevision: 3 });
  vi.mocked(reverser.make).mockReset();
  vi.mocked(reverser.stop).mockClear();
});

describe('Reverse', () => {
  it("is on an imported clip's menu: its footage is written backward, with a pill, then the clip plays from it", async () => {
    let progress: (done: number) => void = () => {};
    let finish: (copy: ReversedCopy) => void = () => {};
    vi.mocked(reverser.make).mockImplementation((_, onProgress) => {
      progress = onProgress;
      return new Promise((resolve) => {
        finish = resolve;
      });
    });
    renderWithProviders(
      <>
        <Timeline />
        <Commands />
        <ReversingPill />
      </>,
    );
    await menuOf(/^broll\.mp4/);
    await userEvent.click(screen.getByRole('menuitem', { name: /^Reverse/ }));
    await frames();
    expect(reverser.make).toHaveBeenCalledWith('b', expect.any(Function));
    act(() => progress(0.42));
    expect(screen.getByRole('status', { name: 'Reversing broll.mp4, 42%' })).toHaveTextContent('Reversing broll.mp4');
    // While it is made, Reverse on that clip is off, saying why.
    act(() => useSelection.setState({ selectedId: 'b' }));
    expect(reverse()).toMatchObject({ enabled: false, reason: 'This clip is already being reversed.' });
    await act(async () => finish({ asset: copyAsset, fromUs: 0, toUs: 10 * S }));
    await frames();
    expect(bridge.apply).toHaveBeenCalledWith({
      label: 'undo.reverse',
      ops: [
        { type: 'addAssets', assets: [copyAsset] },
        { type: 'reverseClip', clipId: 'b', copy: { assetId: 'broll-rev', fromUs: 0, toUs: 10 * S } },
      ],
    });
    expect(screen.queryByRole('status', { name: /^Reversing/ })).toBeNull();
  });

  it('plays a reversed clip forward again at once, from its own file', async () => {
    useProject.setState({ snapshot: reversed() });
    useSelection.setState({ selectedId: 'b' });
    renderWithProviders(<Commands />);
    expect(reverse()).toMatchObject({ label: 'Reverse', enabled: true });
    act(() => reverse()!.run());
    await frames();
    expect(reverser.make).not.toHaveBeenCalled();
    expect(bridge.apply).toHaveBeenCalledWith({
      label: 'undo.reverse',
      ops: [{ type: 'reverseClip', clipId: 'b', copy: null }],
    });
  });

  it('is off for a recording clip, saying why, and not on its menu', async () => {
    renderWithProviders(
      <>
        <Timeline />
        <Commands />
      </>,
    );
    act(() => useSelection.setState({ selectedId: 'a' }));
    expect(reverse()).toMatchObject({ enabled: false, reason: 'Only imported video clips can play backward.' });
    await menuOf(/^Recording 1/);
    expect(screen.queryByRole('menuitem', { name: /^Reverse/ })).toBeNull();
  });

  it("the pill's stop stops it, and nothing goes in", async () => {
    let fail: (error: Error) => void = () => {};
    vi.mocked(reverser.make).mockImplementation(
      () =>
        new Promise((_, reject) => {
          fail = reject;
        }),
    );
    useSelection.setState({ selectedId: 'b' });
    renderWithProviders(
      <>
        <Timeline />
        <Commands />
        <ReversingPill />
      </>,
    );
    act(() => reverse()!.run());
    await frames();
    await userEvent.click(screen.getByRole('button', { name: 'Stop reversing' }));
    expect(reverser.stop).toHaveBeenCalledWith('b');
    await act(async () => fail(new Error('Reversing stopped.', { cause: { code: 'reverse.stopped', params: {} } })));
    await frames();
    expect(bridge.apply).not.toHaveBeenCalled();
    expect(screen.queryByRole('status', { name: /^Reversing broll/ })).toBeNull();
    expect(screen.getByText('Reversing stopped')).toBeInTheDocument();
  });
});
