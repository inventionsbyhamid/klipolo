import { act, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fixtureSnapshot } from '../test/fixtures';
import { renderWithProviders } from '../test/render';

/*
 * A recording in the Library: one row named as the timeline names it ("Recording 1"), its length and its sources, and
 * none of its files on their own. Record your screen opens the Record dialog from the Library's head and from an empty
 * project's preview, as from the start screen.
 */

vi.mock('../bridge/commands', () => ({ bridge: { apply: vi.fn() }, mediaUrl: (p: string) => p }));
vi.mock('../bridge/library', () => ({
  library: {
    pack: vi.fn(() => Promise.resolve([])),
    marks: vi.fn(() => Promise.resolve({})),
    picture: vi.fn(() => Promise.reject(new Error('no pictures in tests'))),
    speechRanges: vi.fn(() => Promise.resolve([])),
  },
}));
vi.mock('../bridge/engine', () => ({
  engine: { seek: vi.fn(() => Promise.resolve()), setView: vi.fn(() => Promise.resolve()) },
}));
vi.mock('../bridge/appWindow', () => ({
  appWindow: { fullScreen: vi.fn(() => Promise.resolve(false)), isFullScreen: vi.fn(() => Promise.resolve(false)) },
}));
const { useProject } = await import('../state/project');
const { useUi } = await import('../state/ui');
const { useRecorder } = await import('../state/recorder');
const { recordingFiles, recordingRows } = await import('../features/media/recordings');
const { Library } = await import('./Library');
const { Preview } = await import('../preview/Preview');

const openRecorder = vi.fn(() => Promise.resolve());

beforeEach(() => {
  openRecorder.mockClear();
  useProject.setState({ snapshot: fixtureSnapshot(), pending: 0 });
  useUi.setState({ libraryTab: 'clips' });
  useRecorder.setState({ openRecorder });
});

/** The names a list shows, in order. */
const listed = (list: HTMLElement) =>
  within(list)
    .getAllByRole('listitem')
    .map((li) => li.querySelector('.row-title')!.textContent);

describe('a recording in the Library', () => {
  it('a_recording_is_one_row_and_its_files_are_hidden', () => {
    const { project } = useProject.getState().snapshot!;
    // The fixture's take: its screen, a camera bubble and a mic, a minute long.
    expect(recordingFiles(project)).toEqual(new Set(['screen', 'camera', 'mic']));
    expect(recordingRows(project)).toEqual([
      { id: 'rec', name: 'Recording 1', durationUs: 60_000_000, meta: '1:00, screen, mic and camera' },
    ]);
    renderWithProviders(<Library />);
    const clips = screen.getByRole('list', { name: 'Your clips' });
    expect(listed(clips)).toEqual(['Recording 1', 'broll.mp4']);
    expect(within(clips).queryByText(/of Recording 1/)).toBeNull();
    expect(within(clips).queryByText('screen.mp4')).toBeNull();
    expect(within(clips).queryByText('camera.mp4')).toBeNull();
    // Its mic, a minute long, is no song or sound of its own either.
    for (const tab of ['music', 'sounds'] as const) {
      act(() => useUi.setState({ libraryTab: tab }));
      const panel = screen.getByRole('tabpanel');
      expect(within(panel).queryByText(/Mic/)).toBeNull();
      expect(within(panel).queryByText('mic.mp3')).toBeNull();
    }
    expect(within(screen.getByRole('tabpanel')).getByText('music.mp3')).toBeInTheDocument();
  });

  it('row_meta_names_the_sources', () => {
    const snapshot = useProject.getState().snapshot!;
    const [take] = snapshot.project.recordings;
    const second = { ...take, id: 'rec-2', sources: take.sources.filter((s) => s.role !== 'camera') };
    const third = { ...take, id: 'rec-3', sources: take.sources.filter((s) => s.role === 'screen') };
    snapshot.project.recordings.push(second, third);
    expect(recordingRows(snapshot.project).map((row) => [row.name, row.meta])).toEqual([
      ['Recording 1', '1:00, screen, mic and camera'],
      ['Recording 2', '1:00, screen and mic'],
      ['Recording 3', '1:00, screen'],
    ]);
    renderWithProviders(<Library />);
    const row = (name: string) => screen.getByText(name).closest('li')!;
    expect(within(row('Recording 1')).getByText('1:00, screen, mic and camera')).toBeInTheDocument();
    // The video plays Recording 1, so its row says so too.
    expect(within(row('Recording 1')).getByText('On the timeline')).toBeInTheDocument();
    expect(within(row('Recording 2')).getByText('1:00, screen and mic')).toBeInTheDocument();
    expect(within(row('Recording 2')).queryByText('On the timeline')).toBeNull();
    expect(within(row('Recording 3')).getByText('1:00, screen')).toBeInTheDocument();
  });

  it('a recording’s file the video uses on its own keeps its row, named by its source, with its menu and mark', () => {
    const snapshot = useProject.getState().snapshot!;
    const { main, tracks } = snapshot.project.timeline;
    // The camera added as a clip of its own, and the mic as a sound of its own: both play that file alone.
    main.push({ ...main[1], id: 'c', source: { type: 'asset', id: 'camera' }, sourceOutUs: 5_000_000 });
    const song = tracks[1].items[0];
    if (song.content.type !== 'audio') throw new Error('the fixture’s song is a sound');
    tracks[1].items.push({ ...song, id: 'alone', startUs: 40_000_000, content: { ...song.content, assetId: 'mic' } });
    renderWithProviders(<Library />);
    const clips = screen.getByRole('list', { name: 'Your clips' });
    expect(listed(clips)).toEqual(['Recording 1', 'broll.mp4', 'Camera of Recording 1']);
    const camera = within(clips).getByText('Camera of Recording 1').closest('li')!;
    expect(within(camera).getByText('On the timeline')).toBeInTheDocument();
    expect(within(camera).getByRole('button', { name: 'More for Camera of Recording 1' })).toBeInTheDocument();
    expect(within(camera).getByRole('button', { name: 'Add Camera of Recording 1 to the end' })).toBeInTheDocument();
    // The screen plays only in the recording's own clip: still hidden.
    expect(within(clips).queryByText('Screen of Recording 1')).toBeNull();
    act(() => useUi.setState({ libraryTab: 'sounds' }));
    const mic = within(screen.getByRole('tabpanel')).getByText('Mic of Recording 1').closest('li')!;
    expect(within(mic).getByText('On the timeline')).toBeInTheDocument();
  });

  it('a recording’s sound separated onto its own track (tied to its recording) stays hidden', () => {
    const snapshot = useProject.getState().snapshot!;
    const song = snapshot.project.timeline.tracks[1].items[0];
    if (song.content.type !== 'audio') throw new Error('the fixture’s song is a sound');
    snapshot.project.timeline.tracks[1].items.push({
      ...song,
      id: 'separated',
      content: { ...song.content, assetId: 'mic', tie: { recording: 'rec', source: 'mic' } },
    });
    useUi.setState({ libraryTab: 'sounds' });
    renderWithProviders(<Library />);
    expect(within(screen.getByRole('tabpanel')).queryByText(/Mic/)).toBeNull();
  });

  it('a recording whose main footage is gone has no star to set, as no file is there to mark', () => {
    const snapshot = useProject.getState().snapshot!;
    snapshot.project.assets = snapshot.project.assets.filter((a) => a.id !== 'screen');
    renderWithProviders(<Library />);
    const take = screen.getByText('Recording 1').closest('li')!;
    expect(within(take).queryByRole('button', { name: /^Favorite/ })).toBeNull();
    // A file's row keeps its star.
    const broll = screen.getByText('broll.mp4').closest('li')!;
    expect(within(broll).getByRole('button', { name: 'Favorite broll.mp4' })).toBeInTheDocument();
  });

  it('a game take’s row names its game where a screen take names its screen', () => {
    const [take] = useProject.getState().snapshot!.project.recordings;
    const asGame = (id: string, keep: (role: string) => boolean) => ({
      ...take,
      id,
      kind: 'gameplay' as const,
      sources: take.sources
        .filter((s) => keep(s.role))
        .map((s) => (s.role === 'screen' ? { ...s, role: 'game' as const } : s)),
    });
    const rows = recordingRows({
      recordings: [
        asGame('g1', () => true),
        asGame('g2', (role) => role !== 'camera'),
        asGame('g3', (role) => role === 'screen'),
        asGame('g4', (role) => role !== 'mic'),
      ],
    });
    expect(rows.map((row) => row.meta)).toEqual([
      '1:00, game, mic and camera',
      '1:00, game and mic',
      '1:00, game',
      '1:00, game and camera',
    ]);
  });

  it('a recording the project doesn’t have, or one with no length, says so as a recording', async () => {
    const { addRecording, toTheEnd } = await import('../features/media/addMedia');
    const { useToasts } = await import('../state/toasts');
    useToasts.setState({ toasts: [] });
    const texts = () => useToasts.getState().toasts.map((toast) => toast.text);
    act(() => addRecording('gone', toTheEnd));
    await waitFor(() => expect(texts()).toEqual(["That recording isn't in this project."]));
    const [take] = useProject.getState().snapshot!.project.recordings;
    for (const source of take.sources) for (const run of source.runs) Object.assign(run, { offsetUs: 0, durationUs: 0 });
    act(() => addRecording('rec', toTheEnd));
    await waitFor(() =>
      expect(texts()).toEqual(["That recording isn't in this project.", 'This recording is too short to add.']),
    );
  });

  it('a search finds a recording by its name and by its sources', async () => {
    renderWithProviders(<Library />);
    const search = screen.getByRole('searchbox', { name: 'Search clips' });
    await userEvent.type(search, 'recording 1');
    expect(listed(screen.getByRole('list', { name: 'Your clips' }))).toEqual(['Recording 1']);
    await userEvent.clear(search);
    await userEvent.type(search, 'camera');
    expect(listed(screen.getByRole('list', { name: 'Your clips' }))).toEqual(['Recording 1']);
  });
});


describe('Record your screen', () => {
  it('record_button_opens_the_recorder', async () => {
    renderWithProviders(<Library />);
    const record = screen.getByRole('button', { name: 'Record your screen' });
    expect(record).toHaveAttribute('title', 'Record your screen (Shift Command R)');
    // In the head, after Add files.
    expect(screen.getByRole('button', { name: 'Add files' }).nextElementSibling).toBe(record);
    await userEvent.click(record);
    expect(openRecorder).toHaveBeenCalledTimes(1);
  });

  it('empty_preview_offers_record_your_screen_as_secondary', async () => {
    const snapshot = useProject.getState().snapshot!;
    snapshot.project.timeline.main = [];
    const { container } = renderWithProviders(<Preview />);
    const choose = screen.getByRole('button', { name: 'Choose files' });
    const record = screen.getByRole('button', { name: 'Record your screen' });
    expect(choose).toHaveClass('btn-primary', 'btn-lg');
    expect(record).toHaveClass('btn-secondary', 'btn-lg');
    expect(choose.nextElementSibling).toBe(record);
    // Still one yellow.
    expect(container.querySelectorAll('.btn-primary')).toHaveLength(1);
    await userEvent.click(record);
    expect(openRecorder).toHaveBeenCalledTimes(1);
  });

  it('Tab reaches the head’s buttons and the empty preview’s in order, as WebKit keeps them (tabindex 0)', async () => {
    const { unmount } = renderWithProviders(<Library />);
    const head = ['Add files', 'Record your screen', 'Collapse library'].map((name) =>
      screen.getByRole('button', { name }),
    );
    // WebKit leaves a plain button out of Tab order unless Full Keyboard Access is on.
    for (const button of head) expect(button).toHaveAttribute('tabindex', '0');
    act(() => head[0].focus());
    await userEvent.tab();
    expect(head[1]).toHaveFocus();
    await userEvent.tab();
    expect(head[2]).toHaveFocus();
    unmount();
    const snapshot = useProject.getState().snapshot!;
    snapshot.project.timeline.main = [];
    renderWithProviders(<Preview />);
    const choose = screen.getByRole('button', { name: 'Choose files' });
    const record = screen.getByRole('button', { name: 'Record your screen' });
    expect(choose).toHaveAttribute('tabindex', '0');
    expect(record).toHaveAttribute('tabindex', '0');
    act(() => choose.focus());
    await userEvent.tab();
    expect(record).toHaveFocus();
  });
});
