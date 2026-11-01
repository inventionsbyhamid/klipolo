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
 * Its search finds a tab's items by name, tag and related word; favorites and recently used items list first.
 */

const answer = (): Promise<SnapshotPatch> => Promise.resolve(revisionPatch(4));
const PACK: PackItem[] = [
  { id: 'whoosh', name: 'Whoosh', category: 'sound', durationUs: 900_000, width: 0, height: 0, tags: ['transition', 'swipe'] },
  { id: 'impact', name: 'Impact', category: 'sound', durationUs: 400_000, width: 0, height: 0, tags: ['impact', 'hit'] },
  { id: 'soft-ambient', name: 'Soft ambient', category: 'music', durationUs: 16_000_000, width: 0, height: 0, tags: ['calm'] },
  { id: 'star', name: 'Star', category: 'sticker', durationUs: 0, width: 1024, height: 1024, tags: ['star', 'gold'] },
];
vi.mock('../../bridge/library', () => ({
  library: {
    marks: vi.fn(() => Promise.resolve({})),
    setFavorite: vi.fn((key: string, on: boolean) => Promise.resolve({ [key]: { favorite: on, usedMs: 0 } })),
    markUsed: vi.fn((key: string) => Promise.resolve({ [key]: { favorite: false, usedMs: Date.now() } })),
    addMusic: vi.fn(() => answer()),
    addSound: vi.fn(() => answer()),
    addImage: vi.fn(() => answer()),
    addText: vi.fn(() => answer()),
    place: vi.fn(() => answer().then((patch) => ({ patch, unplaced: [] }))),
    pack: vi.fn(() => Promise.resolve(PACK)),
    picture: vi.fn(() => Promise.reject(new Error('no pictures in tests'))),
    speechRanges: vi.fn(() => Promise.resolve([])),
  },
}));
vi.mock('../../bridge/commands', () => ({ bridge: { apply: vi.fn() }, mediaUrl: (p: string) => p }));
// A row's drag hands its drop to the test, which lets go where it likes.
vi.mock('../timeline/drag/mediaDrag', async (original) => ({
  ...(await original<typeof import('../timeline/drag/mediaDrag')>()),
  beginMediaDrag: vi.fn(),
}));
const { bridge } = await import('../../bridge/commands');
const { library } = await import('../../bridge/library');
const { useProject } = await import('../../state/project');
const { usePlayback } = await import('../../state/playback');
const { useUi } = await import('../../state/ui');
const { useSelection } = await import('../../state/selection');
const { useAnnouncer } = await import('../timeline/announce');
const { usePack, loadPack, forgetPictures } = await import('./shelves');
const { useMarks, markUsed } = await import('./marks');
const { useToasts } = await import('../../state/toasts');
const { beginMediaDrag } = await import('../timeline/drag/mediaDrag');
const { useEditorShortcuts } = await import('../command-palette/useEditorShortcuts');
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
  useMarks.setState({ marks: {}, asked: false });
  useToasts.setState({ toasts: [] });
  forgetPictures();
});

/** The names a list shows, in order. */
const listed = (list: HTMLElement) =>
  within(list)
    .getAllByRole('listitem')
    .map((li) => li.querySelector('.row-title, .picture-name')!.textContent);

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

  it('with no clips, the Clips tab says what shows there and where the best moments come from', () => {
    const snapshot = useProject.getState().snapshot!;
    snapshot.project.assets = snapshot.project.assets.filter((a) => a.kind !== 'video');
    snapshot.project.recordings = [];
    snapshot.project.timeline.main = [];
    renderWithProviders(<Library />);
    const panel = screen.getByRole('tabpanel');
    expect(within(panel).getByText('No clips yet')).toBeInTheDocument();
    expect(
      within(panel).getByText(
        'Clips you add show up here. Ask chat for the best moments once Klipolo has listened to them.',
      ),
    ).toBeInTheDocument();
    // Nothing lists moments under the clips: the tab never says it does.
    expect(panel).not.toHaveTextContent('lists the best moments');
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
    await act(() => dropPackItem(PACK.find((i) => i.id === 'star')!, { lane: 'picture', trackId: null, atUs: 8_000_000 }));
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

  it('pictures are asked for near the view, a failed one again later, and let go as another project opens', async () => {
    useUi.setState({ libraryTab: 'images' });
    // jsdom has no blob URLs.
    const urls = { create: vi.fn(() => 'blob:logo'), revoke: vi.fn() };
    const had = { create: URL.createObjectURL, revoke: URL.revokeObjectURL };
    Object.assign(URL, { createObjectURL: urls.create, revokeObjectURL: urls.revoke });
    // Tiles off screen ask for nothing until they come near it.
    let seen: (entries: { isIntersecting: boolean }[]) => void = () => undefined;
    vi.stubGlobal(
      'IntersectionObserver',
      class {
        constructor(heard: typeof seen) {
          seen = heard;
        }
        observe() {}
        disconnect() {}
      },
    );
    vi.mocked(library.picture).mockRejectedValueOnce(new Error('busy')).mockResolvedValue(new ArrayBuffer(4));
    const first = renderWithProviders(<Library />);
    expect(library.picture).not.toHaveBeenCalled();
    act(() => seen([{ isIntersecting: true }]));
    await waitFor(() => expect(library.picture).toHaveBeenCalledTimes(1));
    first.unmount();
    vi.unstubAllGlobals();
    // The failed one isn't kept: the next tile asks again, and gets it.
    renderWithProviders(<Library />);
    await waitFor(() => expect(screen.getByText('logo.png').closest('li')!.querySelector('img')).toHaveAttribute('src', 'blob:logo'));
    expect(library.picture).toHaveBeenCalledWith({ path: '/media/logo' });
    // Another project opens: the pictures go (their URLs revoked).
    const other = structuredClone(useProject.getState().snapshot!);
    other.project.id = 'other';
    act(() => useProject.setState({ snapshot: other }));
    await waitFor(() => expect(urls.revoke).toHaveBeenCalledWith('blob:logo'));
    Object.assign(URL, { createObjectURL: had.create, revokeObjectURL: had.revoke });
  });

  it('the pack is read again when it couldn’t be', async () => {
    vi.mocked(library.pack).mockRejectedValueOnce(new Error('busy'));
    loadPack();
    await waitFor(() => expect(usePack.getState().asked).toBe(false));
    loadPack();
    await waitFor(() => expect(usePack.getState().items).toHaveLength(PACK.length));
    expect(library.pack).toHaveBeenCalledTimes(2);
  });

  it('the search finds the tab’s own files and pack items by name, tag and related word', async () => {
    useUi.setState({ libraryTab: 'sounds' });
    renderWithProviders(<Library />);
    const panel = screen.getByRole('tabpanel');
    await screen.findByText('Impact');
    const search = screen.getByRole('searchbox', { name: 'Search sounds' });
    // By name: one of yours.
    await userEvent.type(search, 'kiln');
    expect(within(panel).getByText('kiln-door.wav')).toBeInTheDocument();
    expect(within(panel).queryByText('tick.wav')).toBeNull();
    expect(within(panel).queryByText('Whoosh')).toBeNull();
    // By a tag: the pack's.
    await userEvent.clear(search);
    await userEvent.type(search, 'swipe');
    expect(within(panel).getByText('Whoosh')).toBeInTheDocument();
    expect(within(panel).queryByText('Impact')).toBeNull();
    expect(within(panel).queryByText('Your sounds')).toBeNull();
    // By a related word: "boom" is an impact.
    await userEvent.clear(search);
    await userEvent.type(search, 'boom');
    expect(within(panel).getByText('Impact')).toBeInTheDocument();
    expect(within(panel).queryByText('Whoosh')).toBeNull();
    // Another tab keeps the words and searches its own.
    act(() => useUi.setState({ libraryTab: 'music' }));
    expect(screen.getByRole('searchbox', { name: 'Search music' })).toHaveValue('boom');
    expect(screen.getByRole('tabpanel')).toHaveTextContent('No music matches “boom”.');
  });

  it('Escape clears the search, and a search that finds nothing says so in one line', async () => {
    useUi.setState({ libraryTab: 'sounds' });
    renderWithProviders(<Library />);
    const panel = screen.getByRole('tabpanel');
    await screen.findByText('Impact');
    const search = screen.getByRole('searchbox', { name: 'Search sounds' });
    await userEvent.type(search, 'spaceship');
    expect(within(panel).queryAllByRole('listitem')).toHaveLength(0);
    const none = within(panel).getByText('No sounds match “spaceship”.');
    expect(none.tagName).toBe('P');
    await userEvent.type(search, '{Escape}');
    expect(search).toHaveValue('');
    expect(search).toHaveFocus();
    expect(within(panel).queryByText(/match/)).toBeNull();
    expect(within(panel).getByText('kiln-door.wav')).toBeInTheDocument();
    expect(within(panel).getByText('Impact')).toBeInTheDocument();
    // Clips with no clips at all say there are none, whatever was typed.
    const snapshot = useProject.getState().snapshot!;
    act(() => useUi.setState({ libraryTab: 'clips' }));
    await userEvent.type(screen.getByRole('searchbox', { name: 'Search clips' }), 'zzz');
    expect(screen.getByRole('tabpanel')).toHaveTextContent('No clips match “zzz”.');
    snapshot.project.assets = snapshot.project.assets.filter((a) => a.kind !== 'video');
    act(() => useProject.setState({ snapshot: structuredClone(snapshot) }));
    expect(screen.getByRole('tabpanel')).toHaveTextContent('No clips yet');
  });

  it('typing in the search runs none of the editor’s keys', async () => {
    useUi.setState({ libraryTab: 'sounds' });
    useSelection.setState({ selectedId: 'title' });
    function WithKeys() {
      useEditorShortcuts();
      return <Library />;
    }
    renderWithProviders(<WithKeys />);
    const search = screen.getByRole('searchbox', { name: 'Search sounds' });
    await userEvent.type(search, 's i o j l 1{ArrowLeft}{ArrowRight}{Backspace}{Delete}{Home}{Escape}');
    await act(() => new Promise((r) => setTimeout(r, 40)));
    expect(bridge.apply).not.toHaveBeenCalled();
    expect(useSelection.getState().selectedId).toBe('title');
    expect(usePlayback.getState()).toMatchObject({ playing: false, playheadUs: 2_000_000 });
    expect(useUi.getState().libraryTab).toBe('sounds');
  });

  it('a star makes an item a favorite for every project, and favorites list first on their tab', async () => {
    useUi.setState({ libraryTab: 'sounds' });
    // As Rust kept them: tick.wav starred in another project.
    vi.mocked(library.marks).mockResolvedValueOnce({ 'file:/media/tick': { favorite: true, usedMs: 0 } });
    renderWithProviders(<Library />);
    const panel = screen.getByRole('tabpanel');
    await screen.findByText('Impact');
    const yours = within(panel).getByRole('list', { name: 'Your sounds' });
    await waitFor(() => expect(listed(yours)[0]).toBe('tick.wav'));
    expect(within(yours).getByRole('button', { name: 'Favorite tick.wav' })).toHaveAttribute('aria-pressed', 'true');
    let pack = within(panel).getByRole('list', { name: /^Built in/ });
    expect(listed(pack)).toEqual(['Whoosh', 'Impact']);
    // Starred: saved app-wide, and first.
    const star = within(pack).getByRole('button', { name: 'Favorite Impact' });
    expect(star).toHaveAttribute('aria-pressed', 'false');
    await userEvent.click(star);
    expect(library.setFavorite).toHaveBeenCalledWith('pack:impact', true);
    // First once the pointer has left the list.
    expect(listed(pack)).toEqual(['Whoosh', 'Impact']);
    fireEvent.pointerLeave(pack);
    expect(listed(pack)).toEqual(['Impact', 'Whoosh']);
    expect(within(pack).getByRole('button', { name: 'Favorite Impact' })).toHaveAttribute('aria-pressed', 'true');
    // Starred while searching too.
    await userEvent.type(screen.getByRole('searchbox', { name: 'Search sounds' }), 'hit');
    expect(within(panel).getByRole('button', { name: 'Favorite Impact' })).toHaveAttribute('aria-pressed', 'true');
    await userEvent.clear(screen.getByRole('searchbox', { name: 'Search sounds' }));
    // Taken off again (the search emptied the list on its way, so it is a new one).
    pack = within(panel).getByRole('list', { name: /^Built in/ });
    await userEvent.click(within(pack).getByRole('button', { name: 'Favorite Impact' }));
    expect(library.setFavorite).toHaveBeenLastCalledWith('pack:impact', false);
    fireEvent.pointerLeave(pack);
    expect(listed(pack)).toEqual(['Whoosh', 'Impact']);
    // Pictures and clips have a star as well.
    act(() => useUi.setState({ libraryTab: 'images' }));
    await userEvent.click(await screen.findByRole('button', { name: 'Favorite Star' }));
    expect(library.setFavorite).toHaveBeenLastCalledWith('pack:star', true);
    act(() => useUi.setState({ libraryTab: 'clips' }));
    await userEvent.click(screen.getByRole('button', { name: 'Favorite broll.mp4' }));
    expect(library.setFavorite).toHaveBeenLastCalledWith('file:/media/broll', true);
  });

  it('an answer with no marks reads as none, and a save that answers none keeps the star', async () => {
    useUi.setState({ libraryTab: 'sounds' });
    vi.mocked(library.marks).mockResolvedValueOnce(null as never);
    vi.mocked(library.setFavorite).mockResolvedValueOnce(null as never);
    renderWithProviders(<Library />);
    const star = await screen.findByRole('button', { name: 'Favorite Impact' });
    await waitFor(() => expect(library.marks).toHaveBeenCalled());
    await act(() => Promise.resolve());
    expect(screen.getByText('kiln-door.wav')).toBeInTheDocument();
    await userEvent.click(star);
    await act(() => Promise.resolve());
    expect(screen.getByRole('button', { name: 'Favorite Impact' })).toHaveAttribute('aria-pressed', 'true');
    expect(useToasts.getState().toasts).toHaveLength(0);
  });

  it('a star that couldn’t be saved is taken back, and says so', async () => {
    useUi.setState({ libraryTab: 'sounds' });
    vi.mocked(library.setFavorite).mockRejectedValueOnce(new Error("Klipolo couldn't save your favorites."));
    renderWithProviders(<Library />);
    const star = await screen.findByRole('button', { name: 'Favorite Impact' });
    await userEvent.click(star);
    await waitFor(() => expect(star).toHaveAttribute('aria-pressed', 'false'));
    expect(useToasts.getState().toasts.at(-1)?.text).toBe("Klipolo couldn't save your favorites.");
  });

  it('an item the library adds to the timeline is recently used, and lists first after the favorites', async () => {
    useUi.setState({ libraryTab: 'sounds' });
    vi.mocked(library.marks).mockResolvedValueOnce({
      'file:/media/tick': { favorite: true, usedMs: 0 },
      'file:/media/music': { favorite: false, usedMs: 1_000 },
      'pack:whoosh': { favorite: false, usedMs: 2_000 },
    });
    // Each add moves the project on a revision.
    vi.mocked(library.addSound).mockResolvedValueOnce(revisionPatch(4)).mockResolvedValueOnce(revisionPatch(5));
    renderWithProviders(<Library />);
    const panel = screen.getByRole('tabpanel');
    await screen.findByText('Impact');
    const yours = within(panel).getByRole('list', { name: 'Your sounds' });
    await waitFor(() => expect(listed(yours)).toEqual(['tick.wav', 'music.mp3', 'kiln-door.wav']));
    const pack = within(panel).getByRole('list', { name: /^Built in/ });
    // Added with its "+": used now, so first.
    await userEvent.click(within(pack).getByRole('button', { name: 'Add Impact at the playhead' }));
    await waitFor(() => expect(library.markUsed).toHaveBeenCalledWith('pack:impact'));
    fireEvent.pointerLeave(pack);
    expect(listed(pack)).toEqual(['Impact', 'Whoosh']);
    await userEvent.click(within(yours).getByRole('button', { name: 'Add kiln-door.wav at the playhead' }));
    await waitFor(() => expect(library.markUsed).toHaveBeenLastCalledWith('file:/media/kiln-door'));
    fireEvent.pointerLeave(yours);
    expect(listed(yours)).toEqual(['tick.wav', 'kiln-door.wav', 'music.mp3']);
    // An add that didn't go in uses nothing.
    vi.mocked(library.addSound).mockRejectedValueOnce(new Error('busy'));
    await userEvent.click(within(pack).getByRole('button', { name: 'Add Whoosh at the playhead' }));
    await waitFor(() => expect(library.addSound).toHaveBeenCalledTimes(3));
    await act(() => new Promise((r) => setTimeout(r, 20)));
    expect(library.markUsed).toHaveBeenCalledTimes(2);
  });

  it('an item dragged from the library onto the timeline is recently used too', async () => {
    useUi.setState({ libraryTab: 'sounds' });
    vi.mocked(bridge.apply).mockResolvedValueOnce(revisionPatch(5));
    renderWithProviders(<Library />);
    const drop = (row: HTMLElement) => {
      fireEvent.pointerDown(row, { button: 0, pointerId: 1 });
      return vi.mocked(beginMediaDrag).mock.calls.at(-1)![2];
    };
    const whoosh = (await screen.findByText('Whoosh')).closest('li')!;
    act(() => drop(whoosh)({ lane: 'sound', trackId: null, atUs: 4_000_000, row: 'sounds' }));
    await waitFor(() => expect(library.markUsed).toHaveBeenCalledWith('pack:whoosh'));
    act(() => useUi.setState({ libraryTab: 'clips' }));
    act(() => drop(screen.getByText('broll.mp4').closest('li')!)({ lane: 'main', index: 0 }));
    await waitFor(() => expect(library.markUsed).toHaveBeenLastCalledWith('file:/media/broll'));
  });

  it('a star or a "+" moves no row while the pointer is on the list, so the same place clicks the same item', async () => {
    useUi.setState({ libraryTab: 'sounds' });
    vi.mocked(library.addSound).mockResolvedValueOnce(revisionPatch(4)).mockResolvedValueOnce(revisionPatch(5));
    renderWithProviders(<Library />);
    const panel = screen.getByRole('tabpanel');
    await screen.findByText('Impact');
    const pack = within(panel).getByRole('list', { name: /^Built in/ });
    const moves = new MutationObserver(() => undefined);
    moves.observe(pack, { childList: true });
    const second = () => within(pack).getAllByRole('listitem')[1];
    await userEvent.click(within(second()).getByRole('button', { name: /^Favorite/ }));
    expect(within(second()).getByRole('button', { name: 'Favorite Impact' })).toHaveAttribute('aria-pressed', 'true');
    await userEvent.click(within(second()).getByRole('button', { name: /^Add/ }));
    await waitFor(() => expect(library.markUsed).toHaveBeenCalledWith('pack:impact'));
    await userEvent.click(within(second()).getByRole('button', { name: /^Add/ }));
    await waitFor(() => expect(library.addSound).toHaveBeenCalledTimes(2));
    expect(vi.mocked(library.addSound).mock.calls.map(([source]) => source)).toEqual([
      { type: 'pack', id: 'impact' },
      { type: 'pack', id: 'impact' },
    ]);
    expect(moves.takeRecords()).toHaveLength(0);
    moves.disconnect();
    expect(listed(pack)).toEqual(['Whoosh', 'Impact']);
    // Once the pointer leaves, the list catches up: the favorite first.
    fireEvent.pointerLeave(pack);
    expect(listed(pack)).toEqual(['Impact', 'Whoosh']);
  });

  it('taking a star off with the keyboard keeps the row and the focus where they are', async () => {
    useUi.setState({ libraryTab: 'sounds' });
    vi.mocked(library.marks).mockResolvedValueOnce({ 'file:/media/tick': { favorite: true, usedMs: 0 } });
    renderWithProviders(<Library />);
    const yours = await screen.findByRole('list', { name: 'Your sounds' });
    await waitFor(() => expect(listed(yours)[0]).toBe('tick.wav'));
    const star = within(yours).getByRole('button', { name: 'Favorite tick.wav' });
    act(() => star.focus());
    await userEvent.keyboard(' ');
    expect(library.setFavorite).toHaveBeenCalledWith('file:/media/tick', false);
    expect(star).toHaveAttribute('aria-pressed', 'false');
    expect(listed(yours)[0]).toBe('tick.wav');
    expect(star).toHaveFocus();
    // Focus leaving the list lets it catch up.
    act(() => star.blur());
    expect(listed(yours)).toEqual(['music.mp3', 'kiln-door.wav', 'tick.wav']);
  });

  it('a drag from the list goes on while an add lands and moves no row', async () => {
    useUi.setState({ libraryTab: 'sounds' });
    renderWithProviders(<Library />);
    await screen.findByText('Impact');
    const pack = within(screen.getByRole('tabpanel')).getByRole('list', { name: /^Built in/ });
    const whoosh = within(pack).getByText('Whoosh').closest('li')!;
    const moves = new MutationObserver(() => undefined);
    moves.observe(pack, { childList: true });
    // Whoosh is grabbed; meanwhile an earlier add of Impact lands and marks it used.
    fireEvent.pointerDown(whoosh, { button: 0, pointerId: 1 });
    const drop = vi.mocked(beginMediaDrag).mock.calls.at(-1)![2];
    act(() => markUsed('pack:impact'));
    expect(moves.takeRecords()).toHaveLength(0);
    expect(whoosh.isConnected).toBe(true);
    act(() => drop({ lane: 'sound', trackId: null, atUs: 4_000_000, row: 'sounds' }));
    await waitFor(() => expect(library.place).toHaveBeenCalledWith([{ type: 'pack', id: 'whoosh' }], 4_000_000, { type: 'auto' }));
    moves.disconnect();
  });
});
