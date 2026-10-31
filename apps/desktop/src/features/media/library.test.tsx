import { act, fireEvent, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Asset } from '../../bindings/Asset';
import type { PackItem } from '../../bindings/PackItem';
import type { SnapshotPatch } from '../../bindings/SnapshotPatch';
import { fixtureSnapshot, revisionPatch } from '../../test/fixtures';
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
const { usePack } = await import('./shelves');
const { addMedia } = await import('./addMedia');
const { Library } = await import('../../layout/Library');

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
  it('lists the project’s clips first, and the arrow keys move between its tabs', async () => {
    renderWithProviders(<Library />);
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

  it('Music lists your songs, then the pack’s beds; the "+" adds music from the playhead to the video’s end', async () => {
    useUi.setState({ libraryTab: 'music' });
    renderWithProviders(<Library />);
    const panel = screen.getByRole('tabpanel');
    // The 2 minute song the fixture plays as a sound stays on Sounds; lofi.wav is long enough to be a song.
    expect(within(panel).getByText('Your music')).toBeInTheDocument();
    expect(within(panel).getByText('lofi.wav')).toBeInTheDocument();
    expect(within(panel).queryByText('kiln-door.wav')).toBeNull();
    await screen.findByText('Soft ambient');
    expect(within(panel).getByText('Free to use')).toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'Add lofi.wav at the playhead' }));
    await waitFor(() => expect(library.addMusic).toHaveBeenCalledWith({ type: 'asset', id: 'lofi' }, 2_000_000));
    await userEvent.click(screen.getByRole('button', { name: 'Add Soft ambient at the playhead' }));
    await waitFor(() =>
      expect(library.addMusic).toHaveBeenLastCalledWith({ type: 'pack', id: 'soft-ambient' }, 2_000_000),
    );
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
    // A short sound's length to the tenth, a long one's to the second.
    expect(within(panel).getByText('kiln-door.wav').closest('li')).toHaveTextContent('0:02.0');
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
    expect(library.addSound).not.toHaveBeenCalled();
  });

  it('music added at the video’s end starts from the beginning, where it has room', async () => {
    const { musicStartUs } = await import('./place');
    const { project } = useProject.getState().snapshot!;
    // The fixture's video is 30 s: from 2 s it plays to the end; from the end (after playing it through) from 0.
    expect(musicStartUs(project, 2_000_000)).toBe(2_000_000);
    expect(musicStartUs(project, 30_000_000)).toBe(0);
  });
});
