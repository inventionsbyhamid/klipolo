import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { ItemContent } from '../../bindings/ItemContent';
import type { Op } from '../../bindings/Op';
import type { LabelParams } from '../../lib/labels';
import { fixtureSnapshot } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';

/*
 * A sound or music layer's card: Mute beside its Volume (its own switch, so the volume stays for when it plays again)
 * and a sound effect (None, Telephone, Voice or Warm). Each is one labelled step through the commit queue; a song laid
 * in pieces is one, so both go on every piece.
 */

const apply = vi.fn<(label: string, ops: Op[], params?: LabelParams) => Promise<boolean>>(() =>
  Promise.resolve(true),
);
vi.mock('../../bridge/engine', () => ({
  engine: { preview: vi.fn(() => Promise.resolve()), previewClear: vi.fn(() => Promise.resolve()) },
}));
const { useProject } = await import('../../state/project');
const { useSelection } = await import('../../state/selection');
const { usePlayback } = await import('../../state/playback');
const { useToasts } = await import('../../state/toasts');
const { Inspector } = await import('./Inspector');

type AudioContent = Extract<ItemContent, { type: 'audio' }>;
const queued = expect.objectContaining({ projectId: 'project', revision: 3 });
/** Each op's layer and the content it sets. */
const contents = (call: Parameters<typeof apply>) =>
  call[1].map((op) => {
    const { id, patch } = op as unknown as { id: string; patch: { content: AudioContent } };
    return [id, patch.content] as const;
  });

/** The fixture with the song's content changed by `patch`, and its track muted or not. */
function withSong(patch: Partial<AudioContent>, trackMuted = false) {
  const snapshot = fixtureSnapshot();
  const track = snapshot.project.timeline.tracks[1];
  track.muted = trackMuted;
  const song = track.items[0];
  song.content = { ...(song.content as AudioContent), ...patch };
  return snapshot;
}

/** The fixture's song as music laid in three pieces back to back, as `add_music_ops` lays a short song. */
function bed() {
  const snapshot = fixtureSnapshot();
  const audio = snapshot.project.timeline.tracks[1];
  const [song] = audio.items;
  const music: AudioContent = { ...(song.content as AudioContent), volume: 0.3, duck: -12 };
  audio.items = ['bed', 'bed-2', 'bed-3'].map((id, k) => ({
    ...song,
    id,
    startUs: k * 10_000_000,
    durationUs: 10_000_000,
    content: music,
  }));
  return snapshot;
}

beforeEach(() => {
  vi.clearAllMocks();
  useProject.setState({ snapshot: fixtureSnapshot(), pending: 0, apply });
  useSelection.setState({ selectedId: 'song' });
  usePlayback.setState({ playheadUs: 2_000_000, playing: false });
  useToasts.setState({ toasts: [] });
});

describe('a sound layer’s card', () => {
  it('Mute is its own switch beside the volume: the volume stays, one step', async () => {
    renderWithProviders(<Inspector format="landscape" />);
    const mute = screen.getByRole('button', { name: 'Mute' });
    expect(mute).toHaveAttribute('aria-pressed', 'false');
    await userEvent.click(mute);
    await waitFor(() => expect(apply).toHaveBeenCalledTimes(1));
    expect(apply).toHaveBeenCalledWith('volume.mute.undo', expect.any(Array), { noun: 'sound' }, queued);
    expect(contents(apply.mock.calls[0]).map(([id, c]) => [id, c.muted, c.volume])).toEqual([['song', true, 0.4]]);
  });

  it('unmuted on a muted track it is still silent: a note says why, with Unmute track', async () => {
    useProject.setState({ snapshot: withSong({ muted: true }, true) });
    renderWithProviders(<Inspector format="landscape" />);
    const mute = screen.getByRole('button', { name: 'Mute' });
    expect(mute).toHaveAttribute('aria-pressed', 'true');
    await userEvent.click(mute);
    await waitFor(() => expect(apply).toHaveBeenCalledTimes(1));
    expect(apply.mock.calls[0][0]).toBe('volume.unmute.undo');
    expect(contents(apply.mock.calls[0]).map(([id, c]) => [id, c.muted])).toEqual([['song', false]]);
    await waitFor(() =>
      expect(useToasts.getState().toasts.map((t) => t.text)).toEqual(['The Audio 1 track is muted.']),
    );
    expect(useToasts.getState().toasts[0].actions?.map((a) => a.label)).toEqual(['Unmute track']);
  });

  it('the sound effect is None, Telephone, Voice or Warm, one change', async () => {
    renderWithProviders(<Inspector format="landscape" />);
    await userEvent.click(screen.getByRole('button', { name: 'Sound effect: None' }));
    const menu = screen.getByRole('menu');
    expect(within(menu).getAllByRole('menuitemradio').map((item) => item.textContent)).toEqual([
      'None',
      'Telephone',
      'Voice',
      'Warm',
    ]);
    await userEvent.click(within(menu).getByRole('menuitemradio', { name: 'Voice' }));
    await waitFor(() => expect(apply).toHaveBeenCalledTimes(1));
    expect(apply).toHaveBeenCalledWith('sound.effect.undo', expect.any(Array), undefined, queued);
    expect(contents(apply.mock.calls[0]).map(([id, c]) => [id, c.effect, c.volume])).toEqual([
      ['song', 'klipolo.builtin/audio.voice', 0.4],
    ]);
  });

  it('back to None takes the effect off; one this version lacks stays listed, by its id as a tooltip', async () => {
    useProject.setState({ snapshot: withSong({ effect: 'lib.robots/audio.robot' }) });
    renderWithProviders(<Inspector format="landscape" />);
    await userEvent.click(screen.getByRole('button', { name: 'Sound effect: Missing audio effect' }));
    const missing = screen.getByRole('menuitemradio', { name: 'Missing audio effect' });
    expect(missing).toHaveAttribute('aria-checked', 'true');
    expect(within(missing).getByTitle('lib.robots/audio.robot')).toBeInTheDocument();
    await userEvent.click(screen.getByRole('menuitemradio', { name: 'None' }));
    await waitFor(() => expect(apply).toHaveBeenCalledTimes(1));
    expect(contents(apply.mock.calls[0]).map(([id, c]) => [id, c.effect])).toEqual([['song', null]]);
  });

  it('a song laid in pieces is one: Mute and the sound effect go on every piece', async () => {
    useProject.setState({ snapshot: bed() });
    useSelection.setState({ selectedId: 'bed-2' });
    renderWithProviders(<Inspector format="landscape" />);
    await userEvent.click(screen.getByRole('button', { name: 'Mute' }));
    await waitFor(() => expect(apply).toHaveBeenCalledTimes(1));
    expect(contents(apply.mock.calls[0]).map(([id, c]) => [id, c.muted])).toEqual([
      ['bed', true],
      ['bed-2', true],
      ['bed-3', true],
    ]);
    await userEvent.click(screen.getByRole('button', { name: 'Sound effect: None' }));
    await userEvent.click(screen.getByRole('menuitemradio', { name: 'Warm' }));
    await waitFor(() => expect(apply).toHaveBeenCalledTimes(2));
    expect(contents(apply.mock.calls[1]).map(([id, c]) => [id, c.effect])).toEqual([
      ['bed', 'klipolo.builtin/audio.warm'],
      ['bed-2', 'klipolo.builtin/audio.warm'],
      ['bed-3', 'klipolo.builtin/audio.warm'],
    ]);
  });
});
