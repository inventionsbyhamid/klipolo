import { act, fireEvent, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Asset } from '../../bindings/Asset';
import type { ItemContent } from '../../bindings/ItemContent';
import type { PackItem } from '../../bindings/PackItem';
import type { SnapshotPatch } from '../../bindings/SnapshotPatch';
import { fixturePatch, fixtureSnapshot, revisionPatch } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';

/*
 * The Library's tabs: Clips, Music, Sounds and Images, your files first and then Klipolo's pack, each
 * row's "+" adding it at the playhead through the media commands, and a sound let go on the Music lane added as music.
 */

const answer = (): Promise<SnapshotPatch> => Promise.resolve(revisionPatch(4));
const PACK: PackItem[] = [
  { id: 'whoosh', name: 'Whoosh', category: 'sound', durationUs: 900_000, width: 0, height: 0 },
  { id: 'soft-ambient', name: 'Soft ambient', category: 'music', durationUs: 16_000_000, width: 0, height: 0 },
  { id: 'star', name: 'Star', category: 'sticker', durationUs: 0, width: 1024, height: 1024 },
];
vi.mock('../../bridge/library', () => ({
  library: {
    addMusic: vi.fn(() => answer()),
    addSound: vi.fn(() => answer()),
    addImage: vi.fn(() => answer()),
    addText: vi.fn(() => answer()),
    place: vi.fn(() => answer()),
    pack: vi.fn(() => Promise.resolve(PACK)),
    picture: vi.fn(() => Promise.reject(new Error('no pictures in tests'))),
    speechRanges: vi.fn(() => Promise.resolve([])),
  },
}));
vi.mock('../../bridge/commands', () => ({ bridge: {}, mediaUrl: (p: string) => p }));
const { library } = await import('../../bridge/library');
const { useProject } = await import('../../state/project');
const { usePlayback } = await import('../../state/playback');
const { useUi } = await import('../../state/ui');
const { useSelection } = await import('../../state/selection');
const { useAnnouncer } = await import('../timeline/announce');
const { usePack } = await import('./shelves');
const { addMedia } = await import('./addMedia');
const { Library } = await import('../../layout/Library');

type AudioContent = Extract<ItemContent, { type: 'audio' }>;

const file = (id: string, kind: Asset['kind'], seconds: number, extra: Partial<Asset> = {}): Asset => ({
  id,
  kind,
  path: `/media/${id}`,
  name: `${id}.${kind === 'image' ? 'png' : 'wav'}`,
  durationUs: seconds * 1_000_000,
  width: kind === 'image' ? 512 : 0,
  height: kind === 'image' ? 512 : 0,
  fps: 0,
  hasAudio: kind === 'audio',
  ...extra,
});

beforeEach(() => {
  vi.clearAllMocks();
  const snapshot = fixtureSnapshot();
  snapshot.project.assets.push(
    file('kiln-door', 'audio', 2),
    file('tick', 'audio', 0.03),
    file('lofi', 'audio', 168),
    file('teapot', 'image', 0, { madeWith: 'Muse Image' }),
    // Made before the asset said so: its folder does.
    file('kettle', 'image', 0, { path: '/Movies/Klipolo/Rosa Media/Muse Image/kettle.png' }),
  );
  useProject.setState({ snapshot, pending: 0 });
  usePlayback.setState({ playheadUs: 2_000_000, playing: false });
  useUi.setState({ libraryTab: 'clips' });
  usePack.setState({ items: [], asked: false });
});

describe('the Library', () => {
  it('lists the project’s clips in the order the video plays them, and the arrow keys move between its tabs', async () => {
    const snapshot = useProject.getState().snapshot!;
    // A clip imported later but played first comes first; one not on the timeline comes last.
    const broll = snapshot.project.assets.find((a) => a.id === 'broll')!;
    snapshot.project.assets.push({ ...broll, id: 'intro', name: 'intro.mp4' }, { ...broll, id: 'spare', name: 'spare.mp4' });
    snapshot.project.timeline.main.unshift({ ...snapshot.project.timeline.main.at(-1)!, id: 'i', source: { type: 'asset', id: 'intro' } });
    renderWithProviders(<Library />);
    const names = within(screen.getByRole('list', { name: 'Your clips' }))
      .getAllByRole('listitem')
      .map((li) => li.querySelector('.row-title')!.textContent);
    expect(names.indexOf('intro.mp4')).toBeLessThan(names.indexOf('broll.mp4'));
    expect(names.at(-1)).toBe('spare.mp4');
    const clips = screen.getByRole('tab', { name: 'Clips' });
    expect(clips).toHaveAttribute('aria-selected', 'true');
    expect(screen.getByText('broll.mp4')).toBeInTheDocument();
    clips.focus();
    fireEvent.keyDown(clips, { key: 'ArrowRight' });
    expect(screen.getByRole('tab', { name: 'Music' })).toHaveAttribute('aria-selected', 'true');
    expect(screen.getByRole('tab', { name: 'Music' })).toHaveFocus();
    fireEvent.keyDown(screen.getByRole('tab', { name: 'Music' }), { key: 'ArrowLeft' });
    expect(useUi.getState().libraryTab).toBe('clips');
  });

  it('Music lists your songs, then the pack’s beds; the "+" adds music under the whole video', async () => {
    useUi.setState({ libraryTab: 'music' });
    renderWithProviders(<Library />);
    const panel = screen.getByRole('tabpanel');
    // The 2 minute song the fixture plays as a sound stays on Sounds; lofi.wav is long enough to be a song.
    expect(within(panel).getByText('Your music')).toBeInTheDocument();
    // Each list is named by its heading.
    expect(within(panel).getByRole('list', { name: 'Your music' })).toHaveTextContent('lofi.wav');
    expect(within(panel).getByText('lofi.wav')).toBeInTheDocument();
    expect(within(panel).queryByText('kiln-door.wav')).toBeNull();
    await screen.findByText('Soft ambient');
    expect(within(panel).getByText('Free to use')).toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'Add lofi.wav under the whole video' }));
    // From the start, whatever the playhead: music goes under the whole video, as the tab says.
    await waitFor(() => expect(library.addMusic).toHaveBeenCalledWith({ type: 'asset', id: 'lofi' }, 0));
    await userEvent.click(screen.getByRole('button', { name: 'Add Soft ambient under the whole video' }));
    await waitFor(() => expect(library.addMusic).toHaveBeenLastCalledWith({ type: 'pack', id: 'soft-ambient' }, 0));
  });

  it('Sounds lists short sounds and those played as effects, and the pack’s, marked when the video uses them', async () => {
    useUi.setState({ libraryTab: 'sounds' });
    const snapshot = useProject.getState().snapshot!;
    snapshot.project.assets.push(file('klipolo-pack-whoosh', 'audio', 0.9, { name: 'Whoosh' }));
    snapshot.project.timeline.tracks[1].items.push({
      ...snapshot.project.timeline.tracks[1].items[0],
      id: 'w1',
      startUs: 40_000_000,
      durationUs: 900_000,
      content: {
        type: 'audio',
        assetId: 'klipolo-pack-whoosh',
        sourceInUs: 0,
        volume: 1,
        fadeInUs: 0,
        fadeOutUs: 0,
        effect: null,
      },
    });
    renderWithProviders(<Library />);
    const panel = screen.getByRole('tabpanel');
    expect(within(panel).getByText('kiln-door.wav')).toBeInTheDocument();
    // A short sound's length to the tenth (never under a tenth), a long one's to the second.
    expect(within(panel).getByText('kiln-door.wav').closest('li')).toHaveTextContent('0:02.0');
    expect(within(panel).getByText('tick.wav').closest('li')).toHaveTextContent('0:00.1');
    expect(within(panel).getByText('music.mp3').closest('li')).toHaveTextContent('2:00');
    expect(within(panel).getByText('music.mp3').closest('li')).not.toHaveTextContent('2:00.0');
    // The fixture's song plays as a sound effect (no ducking): it is listed here, "On the timeline".
    const song = within(panel).getByText('music.mp3').closest('li')!;
    expect(song).toHaveTextContent('On the timeline');
    // The pack's own file isn't one of yours.
    const whoosh = (await screen.findByText('Whoosh')).closest('li')!;
    expect(whoosh).toHaveTextContent('On the timeline');
    expect(within(panel).getAllByText('Whoosh')).toHaveLength(1);
    await userEvent.click(within(whoosh).getByRole('button', { name: 'Add Whoosh at the playhead' }));
    await waitFor(() => expect(library.addSound).toHaveBeenCalledWith({ type: 'pack', id: 'whoosh' }, 2_000_000));
  });

  it('Images groups your pictures, those made with Muse Image, and the pack’s stickers', async () => {
    useUi.setState({ libraryTab: 'images' });
    renderWithProviders(<Library />);
    const panel = screen.getByRole('tabpanel');
    const grids = within(panel).getAllByRole('list');
    expect(within(panel).getByText('Your images')).toBeInTheDocument();
    expect(within(grids[0]).getByText('logo.png')).toBeInTheDocument();
    expect(within(panel).getByText('Made with Muse Image')).toBeInTheDocument();
    expect(within(grids[1]).getByText('teapot.png')).toBeInTheDocument();
    expect(within(grids[1]).getByText('kettle.png')).toBeInTheDocument();
    await screen.findByText('Star');
    expect(within(panel).getByText('Drop PNG or JPG files here, or ask in chat for a sticker.')).toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'Add Star at the playhead' }));
    await waitFor(() => expect(library.addImage).toHaveBeenCalledWith({ type: 'pack', id: 'star' }, 2_000_000));
  });

  it('a sound let go on the Music lane is added as music from there; on the Sounds lane it stays an effect', async () => {
    act(() => addMedia('lofi', { lane: 'sound', trackId: null, atUs: 6_000_000, row: 'music' }));
    await waitFor(() => expect(library.addMusic).toHaveBeenCalledWith({ type: 'asset', id: 'lofi' }, 6_000_000));
    expect(library.place).not.toHaveBeenCalled();
    act(() => addMedia('kiln-door', { lane: 'sound', trackId: 'audio-1', atUs: 4_000_000, row: 'sounds' }));
    await waitFor(() => expect(library.place).toHaveBeenCalledTimes(1));
    expect(library.addMusic).toHaveBeenCalledTimes(1);
  });

  it('sounds and pictures dragged onto their lanes are placed as the "+" places them, on the track let go on', async () => {
    const { tracks } = useProject.getState().snapshot!.project.timeline;
    const sounds = tracks.find((t) => t.kind === 'audio')!.id;
    act(() => addMedia('kiln-door', { lane: 'sound', trackId: sounds, atUs: 4_000_000, row: 'sounds' }));
    await waitFor(() =>
      expect(library.place).toHaveBeenCalledWith([{ type: 'asset', id: 'kiln-door' }], 4_000_000, {
        type: 'track',
        id: sounds,
      }),
    );
    act(() => addMedia('teapot', { lane: 'picture', trackId: null, atUs: 2_000_000, newTrack: true }));
    await waitFor(() =>
      expect(library.place).toHaveBeenLastCalledWith([{ type: 'asset', id: 'teapot' }], 2_000_000, {
        type: 'newPicture',
      }),
    );
    // A pack sticker let go on the Overlays lane, likewise.
    const { dropPackItem } = await import('./place');
    await act(() => dropPackItem(PACK[2], { lane: 'picture', trackId: null, atUs: 8_000_000 }));
    expect(library.place).toHaveBeenLastCalledWith([{ type: 'pack', id: 'star' }], 8_000_000, { type: 'auto' });
    // What went in is said: the answer adds nothing here, so nothing is announced or selected.
    expect(library.addSound).not.toHaveBeenCalled();
    expect(library.addImage).not.toHaveBeenCalled();
  });

  it('the "+" says what it added for screen readers, and selects it', async () => {
    useUi.setState({ libraryTab: 'sounds' });
    const now = structuredClone(useProject.getState().snapshot!);
    const audio = now.project.timeline.tracks.find((t) => t.kind === 'audio')!;
    audio.items.push({
      ...audio.items[0],
      id: 'k1',
      startUs: 2_000_000,
      durationUs: 2_000_000,
      content: { ...(audio.items[0].content as AudioContent), assetId: 'kiln-door', duck: undefined },
    });
    now.history = { ...now.history, revision: 4 };
    vi.mocked(library.addSound).mockResolvedValueOnce({ ...fixturePatch(now), baseRevision: 3 });
    renderWithProviders(<Library />);
    await userEvent.click(screen.getByRole('button', { name: 'Add kiln-door.wav at the playhead' }));
    await waitFor(() => expect(useAnnouncer.getState().message).toBe('Added kiln-door.wav as a sound at 0:02'));
    expect(useSelection.getState().selectedId).toBe('k1');
  });
});
