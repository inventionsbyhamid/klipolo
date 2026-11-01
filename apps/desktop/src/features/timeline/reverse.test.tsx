import { act, fireEvent, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Asset } from '../../bindings/Asset';
import type { ReversedCopy } from '../../bindings/ReversedCopy';
import type { Snapshot } from '../../bindings/Snapshot';
import { fixturePatch, fixtureSnapshot, titleItem } from '../../test/fixtures';
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
vi.mock('../../bridge/reverse', () => ({
  reverser: { make: vi.fn(), stop: vi.fn().mockResolvedValue(undefined), discard: vi.fn().mockResolvedValue(undefined) },
}));
const { bridge } = await import('../../bridge/commands');
const { reverser } = await import('../../bridge/reverse');
const { useProject } = await import('../../state/project');
const { usePlayback } = await import('../../state/playback');
const { useSelection } = await import('../../state/selection');
const { useUi } = await import('../../state/ui');
const { useReversing } = await import('../../state/reversing');
const { useToasts } = await import('../../state/toasts');
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
  useToasts.setState({ toasts: [] });
  vi.mocked(bridge.apply).mockReset().mockResolvedValue({ ...fixturePatch(reversed(4)), baseRevision: 3 });
  vi.mocked(reverser.make).mockReset();
  vi.mocked(reverser.stop).mockClear();
  vi.mocked(reverser.discard).mockClear();
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

  /** Starts Reverse on clip b, its copy coming when `finish` is called. */
  async function startReverse() {
    let finish: (copy: ReversedCopy) => void = () => {};
    vi.mocked(reverser.make).mockImplementation(
      () =>
        new Promise((resolve) => {
          finish = resolve;
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
    return (copy: ReversedCopy) => act(async () => finish(copy));
  }

  it('throws away a copy that ends after another project opened, and never puts it in that project', async () => {
    const finish = await startReverse();
    const other = fixtureSnapshot();
    other.project.id = 'other';
    act(() => useProject.setState({ snapshot: other }));
    // The pill went with the project.
    expect(screen.queryByRole('status', { name: /^Reversing/ })).toBeNull();
    await finish({ asset: copyAsset, fromUs: 0, toUs: 10 * S });
    await frames();
    expect(bridge.apply).not.toHaveBeenCalled();
    expect(reverser.discard).toHaveBeenCalledWith(copyAsset.path);
  });

  it('throws away a copy whose clip went while it was made, and says so', async () => {
    const finish = await startReverse();
    const gone = fixtureSnapshot();
    gone.project.timeline.main = gone.project.timeline.main.slice(0, 1);
    act(() => useProject.setState({ snapshot: gone }));
    await finish({ asset: copyAsset, fromUs: 0, toUs: 10 * S });
    await frames();
    expect(bridge.apply).not.toHaveBeenCalled();
    expect(reverser.discard).toHaveBeenCalledWith(copyAsset.path);
    expect(useToasts.getState().toasts.map((m) => m.text)).toContain(
      'broll.mp4 changed while it was reversed, so it plays as before.',
    );
  });

  it('shows two clips being reversed in one pill, with their progress together and one stop for both', async () => {
    renderWithProviders(<ReversingPill />);
    act(() => {
      const jobs = useReversing.getState();
      jobs.start('b', 'broll.mp4');
      jobs.start('c', 'intro.mov');
      jobs.progress('b', 0.5);
      jobs.progress('c', 0.2);
    });
    expect(screen.getByRole('status', { name: 'Reversing 2 clips, 35%' })).toHaveTextContent('Reversing 2 clips');
    await userEvent.click(screen.getByRole('button', { name: 'Stop reversing' }));
    expect(reverser.stop).toHaveBeenCalledWith('b');
    expect(reverser.stop).toHaveBeenCalledWith('c');
  });

  it('says so when a clip with sound comes back silent, and only then', async () => {
    const finish = await startReverse();
    await finish({ asset: { ...copyAsset, hasAudio: false }, fromUs: 0, toUs: 10 * S });
    await frames();
    expect(bridge.apply).toHaveBeenCalled();
    const silent = "broll.mp4 plays backward without sound, because its sound couldn't be reversed.";
    expect(useToasts.getState().toasts.map((m) => m.text)).toContain(silent);
  });

  it('says nothing more when the copy has its sound', async () => {
    const finish = await startReverse();
    await finish({ asset: copyAsset, fromUs: 0, toUs: 10 * S });
    await frames();
    expect(bridge.apply).toHaveBeenCalled();
    expect(useToasts.getState().toasts.map((m) => m.text).join(' ')).not.toMatch(/without sound/);
  });

  it('throws away a copy the model refuses', async () => {
    vi.mocked(bridge.apply).mockRejectedValue(new Error('That clip no longer exists.'));
    const finish = await startReverse();
    await finish({ asset: copyAsset, fromUs: 0, toUs: 10 * S });
    await frames();
    expect(bridge.apply).toHaveBeenCalled();
    expect(reverser.discard).toHaveBeenCalledWith(copyAsset.path);
  });

  it("is off while the clip's sound plays in its own layer, saying why: that layer would keep playing forward", async () => {
    const apart = fixtureSnapshot();
    apart.project.timeline.main[1].muted = true;
    apart.project.timeline.tracks.push({
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
    useProject.setState({ snapshot: apart });
    renderWithProviders(
      <>
        <Timeline />
        <Commands />
      </>,
    );
    act(() => useSelection.setState({ selectedId: 'b' }));
    expect(reverse()).toMatchObject({
      enabled: false,
      reason: "This clip's sound plays in its own layer. Remove that layer before reversing the clip.",
    });
    await menuOf(/^broll\.mp4/);
    expect(screen.getByRole('menuitem', { name: /^Reverse/ })).toHaveAttribute('aria-disabled', 'true');
  });
});
