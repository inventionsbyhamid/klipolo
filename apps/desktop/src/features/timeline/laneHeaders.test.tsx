import { act, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Item } from '../../bindings/Item';
import type { Snapshot } from '../../bindings/Snapshot';
import type { Track } from '../../bindings/Track';
import { fixtureSnapshot } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';

/*
 * The lanes' headers: Overlays, Sound effects and Music each have their options (Hide or Mute, Solo, Lock) and a Hide
 * or Mute button in the gutter, which act on every track the lane draws, in one step each.
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

/** The last committed batch: its label, the label's values and its ops. */
const lastApply = async () => {
  await vi.waitFor(() => expect(bridge.apply).toHaveBeenCalled());
  const [tx] = vi.mocked(bridge.apply).mock.calls.at(-1)!;
  return { label: tx.label, params: tx.params, ops: tx.ops };
};

const lane = (name: string) => screen.getByRole('group', { name });
const OVERLAYS = { lane: { code: 'timeline.lane.overlays', params: {} } };
const MUSIC = { lane: { code: 'timeline.lane.music', params: {} } };

/**
 * The fixture (a title on Overlay 1, a sound effect on Audio 1) with a zoom track over the title, and a song on a
 * track of its own.
 */
function lanesSnapshot(change: (tracks: Track[]) => void = () => {}): Snapshot {
  const snapshot = fixtureSnapshot();
  const { tracks } = snapshot.project.timeline;
  const [overlay, audio] = tracks;
  const zoom: Item = {
    ...overlay.items[0],
    id: 'zoom-1',
    attach: null,
    content: { type: 'zoom', target: { type: 'auto' }, scale: 2 },
  };
  const song: Item = { ...audio.items[0], id: 'song-2', startUs: 2_000_000, durationUs: 8_000_000 };
  if (song.content.type === 'audio') song.content = { ...song.content, duck: 0 };
  snapshot.project.timeline.tracks = [
    overlay,
    { ...overlay, id: 'z1', kind: 'zoom', name: 'Zoom 1', items: [zoom] },
    audio,
    { ...audio, id: 'm1', name: 'Audio 2', items: [song] },
  ];
  change(snapshot.project.timeline.tracks);
  return snapshot;
}

function show(snapshot: Snapshot) {
  useProject.setState({ snapshot, pending: 0 });
  return renderWithProviders(<Timeline />);
}

beforeEach(() => {
  usePlayback.setState({ playheadUs: 0, playing: false });
  useSelection.setState({ selectedId: null });
  useUi.setState({ pxPerSecond: 40, snapping: true });
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

describe("a lane's header", () => {
  it('Overlays, Sound effects and Music each have one named after the lane: its options, and Hide or Mute', () => {
    show(lanesSnapshot());
    for (const [name, toggle] of [
      ['Overlays', 'Hide Overlays'],
      ['Sound effects', 'Mute Sound effects'],
      ['Music', 'Mute Music'],
    ]) {
      const head = within(lane(name)).getByRole('toolbar', { name });
      expect(within(head).getByRole('button', { name: `${name} options` })).toBeInTheDocument();
      // Its name says what it does next, so it has no pressed state as well.
      expect(within(head).getByRole('button', { name: toggle })).not.toHaveAttribute('aria-pressed');
    }
    // The Video lane has no track of its own: its icon only.
    expect(within(lane('Video')).queryByRole('toolbar')).toBeNull();
  });

  it('an empty lane keeps its icon: nothing on it to hide or mute', () => {
    show(fixtureSnapshot());
    expect(within(lane('Music')).queryByRole('toolbar')).toBeNull();
    expect(within(lane('Sound effects')).getByRole('toolbar', { name: 'Sound effects' })).toBeInTheDocument();
  });

  it('Hide Overlays hides every picture track, the zoom track too, in one step', async () => {
    show(lanesSnapshot());
    await userEvent.click(screen.getByRole('button', { name: 'Hide Overlays' }));
    expect(await lastApply()).toEqual({
      label: 'timeline.lane.hide',
      params: OVERLAYS,
      ops: [
        { type: 'updateTrack', id: 't1', patch: { hidden: true } },
        { type: 'updateTrack', id: 'z1', patch: { hidden: true } },
      ],
    });
  });

  it('once every picture track is hidden, it reads Show Overlays and shows them all again', async () => {
    show(lanesSnapshot((tracks) => tracks.forEach((t) => (t.hidden = t.kind !== 'audio'))));
    await userEvent.click(screen.getByRole('button', { name: 'Show Overlays' }));
    const { label, ops } = await lastApply();
    expect(label).toBe('timeline.lane.show');
    expect(ops).toEqual([
      { type: 'updateTrack', id: 't1', patch: { hidden: false } },
      { type: 'updateTrack', id: 'z1', patch: { hidden: false } },
    ]);
  });

  it('Mute Music mutes the tracks that hold music, never those of sound effects', async () => {
    show(lanesSnapshot());
    await userEvent.click(screen.getByRole('button', { name: 'Mute Music' }));
    expect(await lastApply()).toEqual({
      label: 'timeline.lane.mute',
      params: MUSIC,
      ops: [{ type: 'updateTrack', id: 'm1', patch: { muted: true } }],
    });
  });

  it("the options list Hide, Solo and Lock, worded for the lane's state, each one step", async () => {
    show(lanesSnapshot((tracks) => (tracks[0].locked = tracks[1].locked = true)));
    await userEvent.click(screen.getByRole('button', { name: 'Overlays options' }));
    const items = screen.getAllByRole('menuitem').map((item) => item.textContent);
    expect(items).toEqual(['Hide Overlays', 'Solo Overlays', 'Unlock Overlays', 'Show all tracks']);
    await userEvent.click(screen.getByRole('menuitem', { name: 'Solo Overlays' }));
    // A zoom track has no solo.
    expect(await lastApply()).toEqual({
      label: 'timeline.lane.solo',
      params: OVERLAYS,
      ops: [{ type: 'updateTrack', id: 't1', patch: { solo: true } }],
    });
  });

  it('Lock Music locks its tracks', async () => {
    show(lanesSnapshot());
    await userEvent.click(screen.getByRole('button', { name: 'Music options' }));
    await userEvent.click(screen.getByRole('menuitem', { name: 'Lock Music' }));
    expect(await lastApply()).toEqual({
      label: 'timeline.lane.lock',
      params: MUSIC,
      ops: [{ type: 'updateTrack', id: 'm1', patch: { locked: true } }],
    });
  });

  it('at rest, the options say what the lane is: muted, soloed, locked', () => {
    show(lanesSnapshot((tracks) => Object.assign(tracks[3], { muted: true, solo: true, locked: true })));
    expect(screen.getByRole('button', { name: 'Music options' })).toHaveAccessibleDescription(
      'Muted, soloed and locked',
    );
    expect(screen.getByRole('button', { name: 'Unmute Music' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Overlays options' })).not.toHaveAccessibleDescription();
  });

  it('while any track is soloed, every lane offers Turn off solo, which unsolos them all in one step', async () => {
    show(lanesSnapshot((tracks) => (tracks[3].solo = true)));
    await userEvent.click(screen.getByRole('button', { name: 'Overlays options' }));
    await userEvent.click(screen.getByRole('menuitem', { name: 'Turn off solo' }));
    expect(await lastApply()).toEqual({
      label: 'timeline.track.soloOff',
      params: undefined,
      ops: [{ type: 'updateTrack', id: 'm1', patch: { solo: false } }],
    });
  });

  it('its blocks show what the lane is: dimmed while hidden, marked while locked', () => {
    const { container } = show(lanesSnapshot((tracks) => (tracks[0].hidden = tracks[0].locked = true)));
    const title = container.querySelector<HTMLElement>('[data-block-id="title"]')!;
    expect(title).toHaveClass('is-dimmed');
    expect(title).toHaveAccessibleName(/locked$/);
    act(() => useProject.setState({ snapshot: lanesSnapshot(), pending: 0 }));
    expect(container.querySelector('[data-block-id="title"]')).not.toHaveClass('is-dimmed');
  });
});
