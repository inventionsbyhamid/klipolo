import { act, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fixtureSnapshot } from '../test/fixtures';
import { renderWithProviders } from '../test/render';

/*
 * A recording in the Library: one row named as the timeline names it ("Recording 1"), its length and its sources, and
 * none of its files on their own. Record your screen opens the Record dialog from the Library's head, as from the start
 * screen.
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
const { useProject } = await import('../state/project');
const { useUi } = await import('../state/ui');
const { recordingFiles, recordingRows } = await import('../features/media/recordings');
const { Library } = await import('./Library');

beforeEach(() => {
  useProject.setState({ snapshot: fixtureSnapshot(), pending: 0 });
  useUi.setState({ libraryTab: 'clips' });
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

