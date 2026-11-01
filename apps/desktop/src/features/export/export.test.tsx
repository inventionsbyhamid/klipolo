import { act, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { ExportEvent } from '../../bindings/ExportEvent';
import type { ExportJobInfo } from '../../bindings/ExportJobInfo';
import type { ExportPlan } from '../../bindings/ExportPlan';
import type { Op } from '../../bindings/Op';
import type { LabelParams } from '../../lib/labels';
import { fixtureSnapshot } from '../../test/fixtures';
import { renderWithProviders } from '../../test/render';

const apply = vi.fn<(label: string, ops: Op[], params?: LabelParams) => Promise<boolean>>(() =>
  Promise.resolve(true),
);
/** What Rust plans: 140 MB a file at High, 80 MB at Smaller file, in Movies / Klipolo. */
const planned = vi.hoisted(
  () => (formats: string[], quality: string) =>
    Promise.resolve({
      files: formats.map((format) => ({
        format,
        width: format === 'portrait' ? 1080 : 1920,
        height: format === 'landscape' ? 1080 : 1920,
        name: `Billing tutorial ${format}.mp4`,
        path: `/m/${format}.mp4`,
      })),
      bytes: (quality === 'standard' ? 80_000_000 : 140_000_000) * formats.length,
      seconds: 30,
      folder: 'Movies / Klipolo',
      slowMaster: true,
      canSaveCaptions: false,
      maximumAsH264: false,
    }),
);
vi.mock('../../bridge/exporter', () => ({
  exporter: {
    plan: vi.fn(planned),
    start: vi.fn(() => Promise.resolve()),
    cancel: vi.fn(() => Promise.resolve()),
    reveal: vi.fn(() => Promise.resolve()),
    share: vi.fn(() => Promise.resolve()),
    open: vi.fn(() => Promise.resolve()),
    files: vi.fn(() => Promise.resolve([{ name: 'Billing tutorial 16x9 2.mp4', bytes: 148_000_000 }])),
    pickFolder: vi.fn(() => Promise.resolve('Desktop / Clips')),
    jobs: vi.fn(() => Promise.resolve([])),
    pause: vi.fn(() => Promise.resolve()),
    resume: vi.fn(() => Promise.resolve()),
    discard: vi.fn(() => Promise.resolve()),
  },
}));
const { exporter } = await import('../../bridge/exporter');
const { useProject } = await import('../../state/project');
const { useExport } = await import('../../state/export');
const { useToasts } = await import('../../state/toasts');
const { commitLatest } = await import('../../state/commit');
const { useAnalysis } = await import('../../state/analysis');
const { ExportButton } = await import('./ExportButton');
const { ExportDialog } = await import('./ExportDialog');
const { roughly } = await import('./presets');

const landscape = {
  format: 'landscape' as const,
  width: 1920,
  height: 1080,
  name: 'Billing tutorial 16x9.mp4',
  path: '/m/Billing tutorial 16x9.mp4',
};
const portrait = {
  format: 'portrait' as const,
  width: 1080,
  height: 1920,
  name: 'Billing tutorial 9x16.mp4',
  path: '/m/Billing tutorial 9x16.mp4',
};

const emit = (...events: ExportEvent[]) => act(() => events.forEach((e) => useExport.getState().onEvent(e)));
const started: ExportEvent = {
  type: 'started',
  jobId: 'job',
  files: [landscape],
  totalFrames: 100,
  segmentsTotal: 1,
};
const progress = (frame: number, etaS: number | null, totalFrames = 100): ExportEvent => ({
  type: 'progress',
  frame,
  totalFrames,
  etaS,
  segmentsDone: 0,
  segmentsTotal: 1,
});
const status = () => screen.getByRole('status');
const coded = (code: string) => ({ code, params: {} });
const DISK_FULL = 'The disk is full. Free up space, then resume.';

/** An unfinished export as `export_jobs` lists it: this project's, paused, 3 of its 10 parts done. */
function job(patch: Partial<ExportJobInfo> = {}): ExportJobInfo {
  return {
    jobId: 'old',
    projectId: 'project',
    projectName: 'Billing tutorial',
    quality: 'maximum',
    files: [landscape],
    state: 'paused',
    message: null,
    frame: 30,
    totalFrames: 100,
    segmentsDone: 3,
    segmentsTotal: 10,
    stale: false,
    createdMs: 0,
    bytes: 0,
    otherBuild: false,
    compatibility: null,
    ...patch,
  };
}

/** Opens the dialog from the top bar's Export, named for the unfinished exports it lists. */
async function openWithJobs(count: number) {
  renderExport();
  const name = count === 1 ? 'Export, 1 unfinished export' : `Export, ${count} unfinished exports`;
  await userEvent.click(await screen.findByRole('button', { name }));
}

/** The unfinished row that says `text`. */
const rowOf = (text: string) => screen.getByText(text).closest('li')!;
const dialog = () => screen.getByRole('dialog');

function renderExport() {
  return renderWithProviders(
    <>
      <ExportButton />
      <ExportDialog />
    </>,
  );
}

/** Opens the dialog from the top bar and exports what is checked. */
async function openAndExport() {
  renderExport();
  await userEvent.click(screen.getByRole('button', { name: 'Export' }));
  await userEvent.click(await screen.findByRole('button', { name: 'Export 1 video' }));
  emit(started);
}

/** What Rust plans once some clip's words are heard in full: a 16:9 video can also save its captions as a file. */
const withWords = async (formats: string[], quality: string) => ({
  ...(await planned(formats, quality)),
  canSaveCaptions: true,
});
const heard = { state: 'done', approximate: false } as const;

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(exporter.plan).mockImplementation(planned as typeof exporter.plan);
  vi.mocked(exporter.jobs).mockResolvedValue([]);
  useProject.setState({ snapshot: fixtureSnapshot(), pending: 0, apply });
  useExport.getState().reset();
  useExport.setState({
    open: false,
    quality: 'high',
    folder: null,
    captionFile: { save: false, kind: 'srt' },
    jobs: [],
    startingOver: false,
  });
  useAnalysis.setState({ states: {} });
  localStorage.clear();
});

describe('export dialog', () => {
  it('checks the format shown, counts what Export makes, and says why when none is picked', async () => {
    renderExport();
    await userEvent.click(screen.getByRole('button', { name: 'Export' }));
    const formats = within(dialog()).getByRole('group', { name: 'Formats' });
    expect(within(formats).getByRole('checkbox', { name: '16:9, 1920 x 1080' })).toBeChecked();
    const vertical = within(formats).getByRole('checkbox', { name: '9:16, 1080 x 1920' });
    expect(vertical).not.toBeChecked();
    await userEvent.click(vertical);
    expect(vertical).toBeChecked();
    expect(await screen.findByText('2 videos, about 280 MB. Takes less than a minute.')).toBeInTheDocument();
    expect(exporter.plan).toHaveBeenLastCalledWith(['landscape', 'portrait'], 'high');
    expect(screen.getByRole('button', { name: 'Export 2 videos' })).toBeInTheDocument();
    await userEvent.click(within(formats).getByRole('checkbox', { name: /^16:9/ }));
    await userEvent.click(vertical);
    const none = screen.getByRole('button', { name: 'Export' , description: 'Pick at least one format' });
    expect(none).toHaveAttribute('aria-disabled', 'true');
    await userEvent.click(none);
    expect(exporter.start).not.toHaveBeenCalled();
    // Picking a format to export never changes the project.
    expect(apply).not.toHaveBeenCalled();
  });

  it('says when captions are still being made for a clip in the video, and still exports', async () => {
    const captioned = fixtureSnapshot();
    captioned.project.captions = { ...captioned.project.captions, enabled: true };
    useProject.setState({ snapshot: captioned });
    // The B-roll clip's words are still being timed; a clip that isn't in the video doesn't count.
    useAnalysis.setState({
      states: {
        broll: { state: 'running', step: 'words', pct: 40 },
        music: { state: 'running', step: 'transcript', pct: 10 },
      },
    });
    renderExport();
    await userEvent.click(screen.getByRole('button', { name: 'Export' }));
    const note = 'Captions are still being made for 1 clip. Exporting now leaves its captions out.';
    expect(screen.getByText(note)).toBeInTheDocument();
    const go = screen.getByRole('button', { name: 'Export 1 video' });
    expect(go).toHaveAccessibleDescription(note);
    // Done, or captions off: nothing to say.
    act(() => useAnalysis.setState({ states: { broll: { state: 'done', approximate: false } } }));
    expect(screen.queryByText(/still being made/)).toBeNull();
    await userEvent.click(go);
    expect(exporter.start).toHaveBeenCalledWith(['landscape'], 'high', expect.any(Function));
  });

  it('quality defaults to High, and Smaller file plans smaller files', async () => {
    renderExport();
    await userEvent.click(screen.getByRole('button', { name: 'Export' }));
    expect(screen.getByRole('radio', { name: 'High' })).toBeChecked();
    expect(await screen.findByText('1 video, about 140 MB. Takes less than a minute.')).toBeInTheDocument();
    await userEvent.click(screen.getByRole('radio', { name: 'Smaller file' }));
    expect(await screen.findByText('1 video, about 80 MB. Takes less than a minute.')).toBeInTheDocument();
    expect(exporter.plan).toHaveBeenLastCalledWith(['landscape'], 'standard');
    await userEvent.click(screen.getByRole('button', { name: 'Export 1 video' }));
    expect(exporter.start).toHaveBeenCalledWith(['landscape'], 'standard', expect.any(Function));
  });

  it('offers Maximum and Master, says what each is for, and that Master is slower here', async () => {
    renderExport();
    await userEvent.click(screen.getByRole('button', { name: 'Export' }));
    const quality = within(dialog()).getByRole('radiogroup', { name: 'Quality' });
    expect(within(quality).getAllByRole('radio').map((r) => r.textContent)).toEqual([
      'Smaller file',
      'High',
      'Maximum',
      'Master',
    ]);
    expect(screen.getByText('Best for YouTube and social.')).toBeInTheDocument();
    await userEvent.click(screen.getByRole('radio', { name: 'Smaller file' }));
    expect(screen.getByText('Smaller files that upload well.')).toBeInTheDocument();
    await userEvent.click(screen.getByRole('radio', { name: 'Maximum' }));
    expect(screen.getByText('Near-lossless HEVC for archiving.')).toBeInTheDocument();
    await userEvent.click(screen.getByRole('radio', { name: 'Master' }));
    expect(
      await screen.findByText('ProRes for another editor. Very large files. Slower on this computer.'),
    ).toBeInTheDocument();
    expect(exporter.plan).toHaveBeenLastCalledWith(['landscape'], 'master');
    // The arrow keys walk all four, round from the last to the first.
    await userEvent.keyboard('{ArrowRight}');
    expect(screen.getByRole('radio', { name: 'Smaller file' })).toBeChecked();
    expect(screen.getByRole('radio', { name: 'Smaller file' })).toHaveFocus();
    await userEvent.keyboard('{ArrowLeft}{ArrowLeft}');
    expect(screen.getByRole('radio', { name: 'Maximum' })).toBeChecked();
    await userEvent.keyboard('{ArrowRight}');
    await userEvent.click(screen.getByRole('button', { name: 'Export 1 video' }));
    expect(exporter.start).toHaveBeenCalledWith(['landscape'], 'master', expect.any(Function));
  });

  it('Master on a computer with a media engine says nothing about speed', async () => {
    const fast = async (formats: string[], quality: string) => ({
      ...(await planned(formats, quality)),
      slowMaster: false,
    });
    vi.mocked(exporter.plan).mockImplementation(fast as typeof exporter.plan);
    useExport.setState({ quality: 'master' });
    renderExport();
    await userEvent.click(screen.getByRole('button', { name: 'Export' }));
    expect(await screen.findByText('ProRes for another editor. Very large files.')).toBeInTheDocument();
    expect(screen.queryByText(/Slower on this computer/)).toBeNull();
  });

  it('offers a captions file beside a 16:9 video once words are heard, remembers it and sends it', async () => {
    renderExport();
    await userEvent.click(screen.getByRole('button', { name: 'Export' }));
    await screen.findByText('1 video, about 140 MB. Takes less than a minute.');
    expect(screen.queryByRole('switch', { name: 'Also save captions as a file' })).toBeNull();
    // A clip's words come in while the dialog is open: it plans again and offers the file.
    vi.mocked(exporter.plan).mockImplementation(withWords as typeof exporter.plan);
    act(() => useAnalysis.setState({ states: { broll: heard } }));
    const save = await screen.findByRole('switch', { name: 'Also save captions as a file' });
    expect(save).not.toBeChecked();
    const kind = () => within(dialog()).getByRole('radiogroup', { name: 'Captions file type' });
    expect(within(kind()).getByRole('radio', { name: 'SRT' })).toBeChecked();
    await userEvent.click(save);
    expect(save).toBeChecked();
    await userEvent.click(within(kind()).getByRole('radio', { name: 'WebVTT' }));
    expect(within(kind()).getByRole('radio', { name: 'WebVTT' })).toBeChecked();
    // Kept on this Mac for the next export, whatever the project.
    expect(useExport.getState().captionFile).toEqual({ save: true, kind: 'vtt' });
    expect(JSON.parse(localStorage.getItem('klipolo.captionFile') ?? 'null')).toEqual({ save: true, kind: 'vtt' });
    // Only beside a 16:9 video.
    const formats = within(dialog()).getByRole('group', { name: 'Formats' });
    await userEvent.click(within(formats).getByRole('checkbox', { name: /^9:16/ }));
    await userEvent.click(within(formats).getByRole('checkbox', { name: /^16:9/ }));
    await waitFor(() => expect(screen.queryByRole('switch', { name: 'Also save captions as a file' })).toBeNull());
    await userEvent.click(within(formats).getByRole('checkbox', { name: /^16:9/ }));
    await userEvent.click(await screen.findByRole('button', { name: 'Export 2 videos' }));
    expect(exporter.start).toHaveBeenCalledWith(['landscape', 'portrait'], 'high', expect.any(Function), {
      captionFile: 'vtt',
    });
  });

  it('sends no captions file while none is offered, even when one was kept', async () => {
    useExport.setState({ captionFile: { save: true, kind: 'srt' } });
    renderExport();
    await userEvent.click(screen.getByRole('button', { name: 'Export' }));
    await screen.findByText('1 video, about 140 MB. Takes less than a minute.');
    expect(screen.queryByRole('switch', { name: 'Also save captions as a file' })).toBeNull();
    await userEvent.click(screen.getByRole('button', { name: 'Export 1 video' }));
    expect(exporter.start).toHaveBeenCalledWith(['landscape'], 'high', expect.any(Function));
  });

  it('a finished export lists its captions file with Show in Finder', async () => {
    vi.mocked(exporter.plan).mockImplementation(withWords as typeof exporter.plan);
    useExport.setState({ captionFile: { save: true, kind: 'srt' } });
    renderExport();
    await userEvent.click(screen.getByRole('button', { name: 'Export' }));
    await screen.findByRole('switch', { name: 'Also save captions as a file', checked: true });
    await userEvent.click(screen.getByRole('button', { name: 'Export 1 video' }));
    expect(exporter.start).toHaveBeenCalledWith(['landscape'], 'high', expect.any(Function), { captionFile: 'srt' });
    emit(started);
    emit(
      { type: 'fileReady', index: 0, path: landscape.path },
      { type: 'fileReady', index: 1, path: '/m/Billing tutorial 16x9.srt' },
      { type: 'finished' },
    );
    expect(screen.getByRole('heading', { name: 'Exported 1 video' })).toBeInTheDocument();
    const row = screen.getByText('Billing tutorial 16x9.srt').closest('li')!;
    expect(within(row).getByText('SRT captions')).toBeInTheDocument();
    await userEvent.click(within(row).getByRole('button', { name: 'Show in Finder' }));
    expect(exporter.reveal).toHaveBeenCalledWith(1);
  });

  it('Try again on a start Rust refused asks for the captions file again; with a kept job it resumes it', async () => {
    vi.mocked(exporter.plan).mockImplementation(withWords as typeof exporter.plan);
    useExport.setState({ captionFile: { save: true, kind: 'vtt' } });
    vi.mocked(exporter.start).mockRejectedValueOnce(new Error('An export is already running.'));
    renderExport();
    await userEvent.click(screen.getByRole('button', { name: 'Export' }));
    await screen.findByRole('switch', { name: 'Also save captions as a file', checked: true });
    await userEvent.click(screen.getByRole('checkbox', { name: /^9:16/ }));
    await userEvent.click(await screen.findByRole('button', { name: 'Export 2 videos' }));
    // Refused before it had a job: Try again asks for the same files, the captions file with them.
    await userEvent.click(await screen.findByRole('button', { name: 'Try again' }));
    expect(exporter.start).toHaveBeenLastCalledWith(['landscape', 'portrait'], 'high', expect.any(Function), {
      captionFile: 'vtt',
    });
    // Now it has a job: it fails with the 16:9 video in place, and Try again goes on from the kept job.
    const both = { ...started, files: [landscape, portrait] } as ExportEvent;
    emit(both, { type: 'fileReady', index: 0, path: landscape.path }, { type: 'failed', message: 'The export stopped.' });
    await userEvent.click(screen.getByRole('button', { name: 'Try again' }));
    expect(exporter.resume).toHaveBeenCalledWith('job', expect.any(Function));
    expect(exporter.start).toHaveBeenCalledTimes(2);
  });

  it('a captions file that couldn’t be saved is said plainly, and the video is ready', async () => {
    await openAndExport();
    emit(
      { type: 'fileReady', index: 0, path: landscape.path },
      { type: 'captionFileFailed', index: 0, diskFull: true },
      { type: 'finished' },
    );
    const why =
      'The captions file couldn’t be saved because the disk is full. Your video is ready. Free up space, then export again to save it.';
    expect(screen.getByText(why)).toBeInTheDocument();
    expect(screen.getByText('Billing tutorial 16x9.mp4')).toBeInTheDocument();
    expect(status()).toHaveTextContent(`Exported 1 video. ${why}`);
  });

  it('a captions file that failed otherwise, or had nothing to save, says so', async () => {
    await openAndExport();
    emit(
      { type: 'fileReady', index: 0, path: landscape.path },
      { type: 'captionFileFailed', index: 0, diskFull: false },
      { type: 'finished' },
    );
    expect(
      screen.getByText('The captions file couldn’t be saved. Your video is ready. Export again to save it.'),
    ).toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'Done' }));
    await userEvent.click(screen.getByRole('button', { name: 'Export' }));
    await userEvent.click(await screen.findByRole('button', { name: 'Export 1 video' }));
    emit(started, { type: 'fileReady', index: 0, path: landscape.path }, { type: 'captionFileEmpty' }, {
      type: 'finished',
    });
    const none = 'There were no captions to save: none of the transcribed words are in your video.';
    expect(screen.getByText(none)).toBeInTheDocument();
    expect(status()).toHaveTextContent(none);
  });

  it('a captions file that couldn’t be saved is in the toast when the dialog is closed', async () => {
    await openAndExport();
    act(() => useExport.getState().setOpen(false));
    emit(
      { type: 'fileReady', index: 0, path: landscape.path },
      { type: 'captionFileFailed', index: 0, diskFull: false },
      { type: 'finished' },
    );
    expect(useToasts.getState().toasts.at(-1)!.text).toBe(
      'Exported 1 video. The captions file couldn’t be saved. Your video is ready. Export again to save it.',
    );
  });

  it('says how long the export takes on this computer, in minutes or hours', async () => {
    const lasting = (seconds: number) => async (formats: string[], quality: string) => ({
      ...(await planned(formats, quality)),
      seconds,
    });
    vi.mocked(exporter.plan).mockImplementation(lasting(240) as typeof exporter.plan);
    renderExport();
    await userEvent.click(screen.getByRole('button', { name: 'Export' }));
    expect(await screen.findByText('1 video, about 140 MB. Takes about 4 minutes.')).toBeInTheDocument();
    vi.mocked(exporter.plan).mockImplementation(lasting(7_200) as typeof exporter.plan);
    await userEvent.click(screen.getByRole('radio', { name: 'Master' }));
    expect(await screen.findByText('1 video, about 140 MB. Takes about 2 hours.')).toBeInTheDocument();
    expect(roughly(70)).toBe('about a minute');
    expect(roughly(3_600)).toBe('about 60 minutes');
    expect(roughly(5_400)).toBe('about 2 hours');
  });

  it('says when Maximum saves H.264 because this Mac has no 10-bit HEVC encoder', async () => {
    const h264 = async (formats: string[], quality: string) => ({
      ...(await planned(formats, quality)),
      maximumAsH264: quality === 'maximum',
    });
    vi.mocked(exporter.plan).mockImplementation(h264 as typeof exporter.plan);
    renderExport();
    await userEvent.click(screen.getByRole('button', { name: 'Export' }));
    await userEvent.click(screen.getByRole('radio', { name: 'Maximum' }));
    expect(
      await screen.findByText(
        'Near-lossless HEVC for archiving. This Mac can’t encode 10-bit HEVC, so Maximum saves H.264 at a higher bitrate.',
      ),
    ).toBeInTheDocument();
  });

  it('Export pressed before the first plan arrives still sends the kept captions file', async () => {
    vi.mocked(exporter.plan).mockImplementation(() => new Promise(() => {}));
    useExport.setState({ captionFile: { save: true, kind: 'vtt' } });
    renderExport();
    await userEvent.click(screen.getByRole('button', { name: 'Export' }));
    await userEvent.click(screen.getByRole('button', { name: 'Export 1 video' }));
    expect(exporter.start).toHaveBeenCalledWith(['landscape'], 'high', expect.any(Function), {
      captionFile: 'vtt',
    });
  });

  it('Change picks the folder the files go to, and the dialog says it', async () => {
    renderExport();
    await userEvent.click(screen.getByRole('button', { name: 'Export' }));
    expect(await within(dialog()).findByText('Movies / Klipolo')).toBeInTheDocument();
    vi.mocked(exporter.plan).mockImplementation((formats) =>
      Promise.resolve({
        files: [],
        bytes: 140_000_000 * formats.length,
        seconds: 30,
        folder: 'Desktop / Clips',
        slowMaster: false,
        canSaveCaptions: false,
        maximumAsH264: false,
      }),
    );
    await userEvent.click(screen.getByRole('button', { name: 'Change folder' }));
    expect(exporter.pickFolder).toHaveBeenCalled();
    expect(await within(dialog()).findByText('Desktop / Clips')).toBeInTheDocument();
  });

  it('an export waits for an edit still waiting its turn in the commit queue, and includes it', async () => {
    // A pick still landing, and a slider release queued behind it, when Command E is pressed.
    apply.mockImplementationOnce(() => new Promise((resolve) => setTimeout(() => resolve(true), 30)));
    void commitLatest('clip.fades.undo', () => [{ type: 'rename', name: 'fades' }]);
    void commitLatest('volume.undo', () => [{ type: 'rename', name: 'volume' }]);
    await useExport.getState().start(['landscape']);
    expect(apply.mock.calls.map(([label]) => label)).toEqual(['clip.fades.undo', 'volume.undo']);
    expect(apply.mock.invocationCallOrder[1]).toBeLessThan(vi.mocked(exporter.start).mock.invocationCallOrder[0]);
  });

  it('exports, shows one bar and the time left, then the files with focus on Done', async () => {
    renderExport();
    await userEvent.click(screen.getByRole('button', { name: 'Export' }));
    await userEvent.click(await screen.findByRole('button', { name: 'Export 1 video' }));
    expect(exporter.start).toHaveBeenCalledWith(['landscape'], 'high', expect.any(Function));
    expect(screen.getByRole('button', { name: 'Cancel export' })).toHaveFocus();
    emit(started, progress(42, 12));
    expect(screen.getByRole('progressbar', { name: 'Export progress' })).toHaveAttribute('aria-valuenow', '42');
    expect(screen.getByText('Exporting the 16:9 video')).toBeInTheDocument();
    expect(screen.getByText('42%')).toBeInTheDocument();
    expect(screen.getByText('Frame 42 of 100.')).toBeInTheDocument();
    expect(screen.getByText('About 15 seconds left')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Exporting' })).toBeDisabled();
    emit(
      { type: 'finishing' },
      { type: 'fileReady', index: 0, path: '/m/Billing tutorial 16x9 2.mp4' },
      { type: 'finished' },
    );
    await waitFor(() => expect(screen.getByRole('button', { name: 'Done' })).toHaveFocus());
    expect(screen.getByRole('heading', { name: 'Exported 1 video' })).toBeInTheDocument();
    expect(status()).toHaveTextContent('Exported 1 video');
    expect(screen.getByText('Saved in Movies / Klipolo.')).toBeInTheDocument();
    const row = screen.getByText('Billing tutorial 16x9 2.mp4').closest('li')!;
    expect(await within(row).findByText('1920 x 1080, 148 MB')).toBeInTheDocument();
    await userEvent.click(within(row).getByRole('button', { name: 'Open' }));
    expect(exporter.open).toHaveBeenCalledWith(0);
    await userEvent.click(screen.getByRole('button', { name: 'Show in Finder' }));
    expect(exporter.reveal).toHaveBeenCalledWith();
    await userEvent.click(screen.getByRole('button', { name: 'Done' }));
    expect(screen.queryByRole('dialog')).toBeNull();
    expect(useExport.getState().phase).toBe('idle');
  });

  it('two videos show their progress together, then which are done', async () => {
    renderExport();
    await userEvent.click(screen.getByRole('button', { name: 'Export' }));
    await userEvent.click(screen.getByRole('checkbox', { name: /^9:16/ }));
    await userEvent.click(await screen.findByRole('button', { name: 'Export 2 videos' }));
    emit({ ...started, files: [landscape, portrait], totalFrames: 54_036 } as ExportEvent, progress(31_320, 70, 54_036));
    expect(screen.getByText('Exporting 2 videos')).toBeInTheDocument();
    expect(screen.getByText('Frame 15,660 of 27,018.')).toBeInTheDocument();
    expect(screen.getByText('About a minute left')).toBeInTheDocument();
    expect(screen.getByRole('checkbox', { name: /^9:16/ })).toHaveAttribute('aria-disabled', 'true');
    await userEvent.click(screen.getByRole('checkbox', { name: /^1:1/ }));
    expect(screen.getByRole('checkbox', { name: /^1:1/ })).not.toBeChecked();
    emit({ type: 'finishing' }, { type: 'fileReady', index: 0, path: landscape.path });
    expect(screen.getByText('16:9 is done')).toBeInTheDocument();
  });

  it('Cancel export names the run, stops it, and leaves the formats to pick again', async () => {
    await openAndExport();
    emit(progress(60, 20));
    await userEvent.click(screen.getByRole('button', { name: 'Cancel export' }));
    expect(exporter.cancel).toHaveBeenCalledWith('job');
    emit({ type: 'cancelled' });
    expect(status()).toHaveTextContent('Export stopped. Nothing was saved.');
    expect(screen.getByRole('button', { name: 'Export 1 video' })).toBeInTheDocument();
  });

  it('a Cancel Rust refuses is said, and Cancel export is offered again', async () => {
    await openAndExport();
    emit(progress(60, 20));
    // Rust's code (the bridge keeps it as the cause); the page words it, keeping the project's name.
    const rust = { code: 'exports.refused.otherProject', params: { project: 'Launch' } };
    vi.mocked(exporter.cancel).mockRejectedValueOnce(new Error('', { cause: rust }));
    await userEvent.click(screen.getByRole('button', { name: 'Cancel export' }));
    await waitFor(() =>
      expect(useToasts.getState().toasts.map((t) => t.text)).toContain(
        'The export didn’t stop. It’s exporting “Launch”, another project.',
      ),
    );
    expect(useExport.getState().stopping).toBe(false);
    expect(screen.getByRole('button', { name: 'Cancel export' })).not.toHaveAttribute('aria-disabled', 'true');
  });

  it('a full disk pauses calmly: the button reads Paused, the dialog says why, and Resume goes on from the kept job', async () => {
    renderExport();
    await userEvent.click(screen.getByRole('button', { name: 'Export' }));
    await userEvent.click(screen.getByRole('checkbox', { name: /^9:16/ }));
    await userEvent.click(await screen.findByRole('button', { name: 'Export 2 videos' }));
    emit(
      { ...started, files: [landscape, portrait] } as ExportEvent,
      progress(60, 10),
      { type: 'fileReady', index: 0, path: landscape.path },
    );
    act(() => useExport.getState().setOpen(false));
    emit({ type: 'paused', message: coded('exports.diskFull'), compatibility: null });
    expect(useToasts.getState().toasts.at(-1)!.text).toBe(`Export paused. ${DISK_FULL}`);
    const pill = screen.getByRole('button', { name: 'Export paused' });
    expect(pill).toHaveTextContent('Paused');
    await userEvent.click(pill);
    expect(screen.getByText(DISK_FULL)).toBeInTheDocument();
    expect(screen.getByText('Export paused')).toBeInTheDocument();
    expect(screen.getByRole('progressbar', { name: 'Export progress' })).toHaveAttribute('aria-valuenow', '60');
    // The file already in place stays listed.
    expect(screen.getByText('Billing tutorial 16x9.mp4')).toBeInTheDocument();
    const resume = screen.getByRole('button', { name: 'Resume' });
    await waitFor(() => expect(resume).toHaveFocus());
    expect(screen.getByRole('button', { name: 'Discard' })).toBeInTheDocument();
    await userEvent.keyboard('{Enter}');
    expect(exporter.resume).toHaveBeenCalledWith('job', expect.any(Function));
    expect(exporter.start).toHaveBeenCalledTimes(1);
    expect(screen.getByRole('progressbar', { name: 'Export progress' })).toHaveAttribute('aria-valuenow', '60');
    expect(status()).toHaveTextContent('Export resumed.');
  });

  it('Pause, then Paused with Resume focused and announced, then Resume continues the job', async () => {
    await openAndExport();
    emit(progress(30, 10));
    await userEvent.click(screen.getByRole('button', { name: 'Pause' }));
    expect(exporter.pause).toHaveBeenCalledWith('job');
    expect(screen.getByText('Pausing')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Pause' })).toHaveAttribute('aria-disabled', 'true');
    emit({ type: 'paused', message: null, compatibility: null });
    expect(screen.getByText('Export paused')).toBeInTheDocument();
    await waitFor(() => expect(screen.getByRole('button', { name: 'Resume' })).toHaveFocus());
    expect(status()).toHaveTextContent('Export paused.');
    await userEvent.click(screen.getByRole('button', { name: 'Resume' }));
    expect(exporter.resume).toHaveBeenCalledWith('job', expect.any(Function));
    expect(exporter.start).toHaveBeenCalledTimes(1);
    await waitFor(() => expect(screen.getByRole('button', { name: 'Pause' })).toHaveFocus());
    expect(status()).toHaveTextContent('Export resumed.');
  });

  it('Cancel export asks before deleting finished parts: Keep paused pauses, Delete stops', async () => {
    await openAndExport();
    emit({ ...progress(60, 20), segmentsDone: 1 } as ExportEvent);
    await userEvent.click(screen.getByRole('button', { name: 'Cancel export' }));
    expect(screen.getByRole('group', { name: 'Stop and delete the finished parts?' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Keep paused' })).toHaveFocus();
    expect(exporter.cancel).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole('button', { name: 'Keep paused' }));
    expect(exporter.pause).toHaveBeenCalledWith('job');
    expect(screen.getByRole('button', { name: 'Pause' })).toHaveFocus();
    // Asked again, from the keyboard: Delete stops it and deletes its parts.
    await userEvent.click(screen.getByRole('button', { name: 'Cancel export' }));
    await userEvent.keyboard('{Shift>}{Tab}{/Shift}');
    expect(screen.getByRole('button', { name: 'Delete' })).toHaveFocus();
    await userEvent.keyboard('{Enter}');
    expect(exporter.cancel).toHaveBeenCalledWith('job');
    // Stopping: Pause does nothing now.
    act(() => useExport.setState({ pausing: false }));
    await userEvent.click(screen.getByRole('button', { name: 'Pause' }));
    expect(exporter.pause).toHaveBeenCalledTimes(1);
  });

  it('with nothing finished yet, Cancel export stops at once; while it finishes, it can only be kept going', async () => {
    await openAndExport();
    emit(progress(10, 20));
    await userEvent.click(screen.getByRole('button', { name: 'Cancel export' }));
    expect(exporter.cancel).toHaveBeenCalledWith('job');
    act(() => useExport.setState({ stopping: false }));
    emit({ ...progress(100, 0), segmentsDone: 1 } as ExportEvent, { type: 'finishing' });
    const pause = screen.getByRole('button', { name: 'Pause' });
    expect(pause).toHaveAttribute('aria-disabled', 'true');
    expect(pause).toHaveAccessibleDescription('It can’t pause while it finishes.');
    await userEvent.click(screen.getByRole('button', { name: 'Cancel export' }));
    await userEvent.click(screen.getByRole('button', { name: 'Keep exporting' }));
    expect(exporter.pause).not.toHaveBeenCalled();
    expect(exporter.cancel).toHaveBeenCalledTimes(1);
  });

  it('a Pause Rust refuses is said, and Pause is offered again', async () => {
    await openAndExport();
    vi.mocked(exporter.pause).mockRejectedValueOnce(
      new Error('', { cause: coded('exports.refused.notRunning') }),
    );
    await userEvent.click(screen.getByRole('button', { name: 'Pause' }));
    await waitFor(() =>
      expect(useToasts.getState().toasts.map((t) => t.text)).toContain(
        'The export didn’t pause. It isn’t running any more.',
      ),
    );
    expect(screen.getByRole('button', { name: 'Pause' })).not.toHaveAttribute('aria-disabled', 'true');
  });

  it('a pause with no reason says everything finished so far is kept, and Discard deletes the kept job', async () => {
    await openAndExport();
    emit(progress(30, 10), { type: 'paused', message: null, compatibility: null });
    expect(status()).toHaveTextContent('Export paused.');
    expect(screen.getByText('Everything finished so far is kept.')).toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'Discard' }));
    expect(exporter.discard).toHaveBeenCalledWith('job');
    // Back to choosing formats, with nothing kept.
    expect(await screen.findByRole('button', { name: 'Export 1 video' })).toHaveFocus();
    expect(useExport.getState().jobId).toBeNull();
  });

  it('retries say so, and the time left comes back beside the compatibility note', async () => {
    await openAndExport();
    emit({ type: 'retrying' }, { type: 'retrying' }, progress(50, 40));
    expect(screen.getByText('Trying again in compatibility mode. This is slower.')).toBeInTheDocument();
    expect(screen.getByText('About 40 seconds left')).toBeInTheDocument();
  });

  it('a failure before the export started is its project’s: Try again never exports another project', async () => {
    vi.mocked(exporter.start).mockRejectedValueOnce(new Error('Your latest changes couldn’t be saved.'));
    await useExport.getState().start(['landscape']);
    expect(useExport.getState()).toMatchObject({ phase: 'failed', jobId: null });
    // Another project opens: the failure goes with its project, and Try again exports nothing.
    const other = fixtureSnapshot();
    act(() => useProject.setState({ snapshot: { ...other, project: { ...other.project, id: 'b' } } }));
    expect(useExport.getState().phase).toBe('idle');
    await useExport.getState().retry();
    expect(exporter.start).toHaveBeenCalledTimes(1);
  });

  it('closed while exporting, it keeps going: the button shows the progress and opens it again', async () => {
    await openAndExport();
    emit(progress(42, 12));
    await userEvent.keyboard('{Escape}');
    expect(screen.queryByRole('dialog')).toBeNull();
    expect(exporter.cancel).not.toHaveBeenCalled();
    const pill = screen.getByRole('button', { name: 'Exporting 42%' });
    await userEvent.click(pill);
    expect(screen.getByRole('progressbar', { name: 'Export progress' })).toBeInTheDocument();
  });

  it('a run that ends while the dialog is closed says so in a toast', async () => {
    await openAndExport();
    act(() => useExport.getState().setOpen(false));
    emit({ type: 'fileReady', index: 0, path: landscape.path }, { type: 'finished' });
    const done = useToasts.getState().toasts.at(-1)!;
    expect(done.text).toBe('Exported 1 video.');
    done.actions![0].run();
    expect(exporter.reveal).toHaveBeenCalledWith();
  });

  it('Share… opens the share menu for the exported files beside the button, and says so when they are gone', async () => {
    await openAndExport();
    emit({ type: 'fileReady', index: 0, path: landscape.path }, { type: 'finished' });
    const share = await screen.findByRole('button', { name: 'Share…' });
    share.getBoundingClientRect = () => ({ x: 300, y: 500, left: 300, top: 500, width: 90, height: 36 }) as DOMRect;
    await userEvent.click(share);
    expect(exporter.share).toHaveBeenCalledWith({ x: 300, y: 500, width: 90, height: 36 });
    const gone = { code: 'exports.revealGone', params: {} };
    vi.mocked(exporter.share).mockRejectedValueOnce(new Error("That video isn't there any more.", { cause: gone }));
    await userEvent.click(share);
    await waitFor(() => expect(useToasts.getState().toasts.at(-1)?.text).toBe("That video isn't there any more."));
  });

  it('Open and Show in Finder say so when the file is gone', async () => {
    await openAndExport();
    emit({ type: 'fileReady', index: 0, path: landscape.path }, { type: 'finished' });
    const gone = { code: 'exports.revealGone', params: {} };
    vi.mocked(exporter.open).mockRejectedValueOnce(new Error("That video isn't there any more.", { cause: gone }));
    vi.mocked(exporter.reveal).mockRejectedValueOnce(new Error("That video isn't there any more.", { cause: gone }));
    await userEvent.click(screen.getByRole('button', { name: 'Open' }));
    await userEvent.click(screen.getByRole('button', { name: 'Show in Finder' }));
    await waitFor(() =>
      expect(useToasts.getState().toasts.map((t) => t.text).slice(-2)).toEqual([
        "That video isn't there any more.",
        "That video isn't there any more.",
      ]),
    );
  });

  it('the run is said at each quarter while it exports', async () => {
    await openAndExport();
    emit(progress(30, 20));
    expect(status()).toHaveTextContent('25% exported.');
  });

  it('Export is off until there is a video, and says why', async () => {
    useProject.setState({ snapshot: null });
    const { rerender } = renderWithProviders(<ExportButton />);
    // Off but not `disabled`, so WebKit shows why on hover; a press opens nothing.
    expect(screen.getByRole('button', { name: 'Export' })).toHaveAttribute('aria-disabled', 'true');
    expect(screen.getByRole('button', { name: 'Export' })).toBeEnabled();
    expect(screen.getByRole('button', { name: 'Export' }).title).toBe('Add a video first');
    await userEvent.click(screen.getByRole('button', { name: 'Export' }));
    expect(useExport.getState().open).toBe(false);
    useProject.setState({ snapshot: fixtureSnapshot() });
    rerender(<ExportButton />);
    expect(screen.getByRole('button', { name: 'Export' })).not.toHaveAttribute('aria-disabled');
    expect(screen.getByRole('button', { name: 'Export' }).title).toBe('Export (Command E)');
  });
});

describe('unfinished exports', () => {
  it('get a dot on Export, and each row says how far it came, this project’s first', async () => {
    vi.mocked(exporter.jobs).mockResolvedValue([
      job({ jobId: 'other', projectId: 'p2', projectName: 'Vlog', segmentsDone: 1, segmentsTotal: 4 }),
      job(),
    ]);
    await openWithJobs(2);
    const rows = within(screen.getByRole('region', { name: 'Unfinished exports' })).getAllByRole('listitem');
    expect(rows.map((r) => within(r).getAllByText(/parts done/)[0].textContent)).toEqual([
      'Resume export of Billing tutorial: 3 of 10 parts done',
      'Resume export of Vlog: 1 of 4 parts done',
    ]);
    // The formats are still there to export anew below them.
    expect(screen.getByRole('button', { name: 'Export 1 video' })).toBeInTheDocument();
  });

  it('an answer that isn’t a list of jobs lists none, and Export stays as it is', async () => {
    vi.mocked(exporter.jobs).mockResolvedValue(null as unknown as ExportJobInfo[]);
    renderExport();
    await act(() => useExport.getState().loadJobs());
    expect(useExport.getState().jobs).toEqual([]);
    expect(screen.getByRole('button', { name: 'Export' })).toBeInTheDocument();
  });

  it('a crash’s job resumes from its row, from the keyboard', async () => {
    vi.mocked(exporter.jobs).mockResolvedValue([job({ state: 'interrupted' })]);
    await openWithJobs(1);
    const row = rowOf('Resume export of Billing tutorial: 3 of 10 parts done');
    const resume = within(row).getByRole('button', { name: 'Resume' });
    expect(resume).toHaveAccessibleDescription(/Resume export of Billing tutorial: 3 of 10 parts done/);
    resume.focus();
    vi.mocked(exporter.jobs).mockResolvedValue([]);
    await userEvent.keyboard('{Enter}');
    expect(exporter.resume).toHaveBeenCalledWith('old', expect.any(Function));
    // It shows its progress as it was at once, and goes on from there.
    expect(screen.getByRole('progressbar', { name: 'Export progress' })).toHaveAttribute('aria-valuenow', '30');
    expect(useExport.getState()).toMatchObject({ phase: 'running', jobId: 'old', runQuality: 'maximum' });
    expect(screen.getByRole('button', { name: 'Pause' })).toHaveFocus();
  });

  it('a changed project still resumes the version it exported, or starts over, or is discarded', async () => {
    vi.mocked(exporter.jobs).mockResolvedValue([
      job({ stale: true, message: coded('exports.encoderBusy') }),
      job({ jobId: 'vlog', projectId: 'p2', projectName: 'Vlog', stale: true }),
    ]);
    await openWithJobs(2);
    const stale = rowOf('Resume export of Billing tutorial: 3 of 10 parts done');
    // Why it paused comes first, changed project or not.
    expect(
      within(stale).getByText(
        'The video encoder is busy. Another app may be using it, like a screen recorder or a video call. Close it, then resume.',
      ),
    ).toBeInTheDocument();
    expect(within(stale).getByText('This project changed since the export started.')).toBeInTheDocument();
    expect(within(stale).getByText('Resume finishes the version from when you started.')).toBeInTheDocument();
    expect(within(stale).getByRole('button', { name: 'Resume' })).toBeInTheDocument();
    expect(within(stale).getByRole('button', { name: 'Discard' })).toBeInTheDocument();
    // Another project's job can't be started over from here: its project isn't open.
    const other = rowOf('Resume export of Vlog: 3 of 10 parts done');
    expect(within(other).queryByRole('button', { name: 'Start over with your latest changes' })).toBeNull();
    // Start over asks the plan first: held, so a second press while it's under way does nothing.
    let planned = () => undefined as void;
    const plan = vi.mocked(exporter.plan).getMockImplementation()!;
    vi.mocked(exporter.plan).mockImplementationOnce(
      (formats, quality) => new Promise<ExportPlan>((r) => (planned = () => void plan(formats, quality).then(r))),
    );
    const startOver = within(stale).getByRole('button', { name: 'Start over with your latest changes' });
    await userEvent.click(startOver);
    await userEvent.click(startOver);
    await act(async () => planned());
    // Rust swaps them: the kept job goes only once the new export holds the slot and is ready to run.
    await waitFor(() =>
      expect(exporter.start).toHaveBeenCalledWith(['landscape'], 'maximum', expect.any(Function), {
        replaces: 'old',
      }),
    );
    expect(exporter.start).toHaveBeenCalledTimes(1);
    expect(exporter.discard).not.toHaveBeenCalled();
  });

  it('a Start over Rust refuses keeps the kept job, and Try again asks for the same swap', async () => {
    vi.mocked(exporter.jobs).mockResolvedValue([job({ stale: true })]);
    vi.mocked(exporter.start).mockRejectedValueOnce(
      new Error('An export is already running.', { cause: coded('exports.refused.busy') }),
    );
    await openWithJobs(1);
    const row = rowOf('Resume export of Billing tutorial: 3 of 10 parts done');
    await userEvent.click(within(row).getByRole('button', { name: 'Start over with your latest changes' }));
    await waitFor(() => expect(screen.getByRole('alert')).toHaveTextContent('An export is already running.'));
    expect(exporter.discard).not.toHaveBeenCalled();
    expect(useExport.getState().jobs.map((j) => j.jobId)).toEqual(['old']);
    await userEvent.click(screen.getByRole('button', { name: 'Try again' }));
    await waitFor(() => expect(exporter.start).toHaveBeenCalledTimes(2));
    expect(exporter.start).toHaveBeenLastCalledWith(['landscape'], 'maximum', expect.any(Function), {
      replaces: 'old',
    });
  });

  it('a resume Rust refuses as busy says so, and the job stays listed as it was', async () => {
    vi.mocked(exporter.jobs).mockResolvedValue([job({ state: 'failed', message: coded('exports.diskFull') })]);
    vi.mocked(exporter.resume).mockRejectedValueOnce(
      new Error('An export is already running.', { cause: coded('exports.refused.busy') }),
    );
    await openWithJobs(1);
    const row = rowOf('Resume export of Billing tutorial: 3 of 10 parts done');
    expect(within(row).getByText(DISK_FULL)).toBeInTheDocument();
    await userEvent.click(within(row).getByRole('button', { name: 'Resume' }));
    await waitFor(() => expect(screen.getByRole('alert')).toHaveTextContent('An export is already running.'));
    // Try again asks for the same job again; Close puts it back in the list, its reason unchanged.
    await userEvent.click(screen.getByRole('button', { name: 'Try again' }));
    expect(exporter.resume).toHaveBeenLastCalledWith('old', expect.any(Function));
    emit({ type: 'failed', message: coded('exports.diskFull') });
    await userEvent.click(within(dialog()).getByText('Close'));
    expect(useExport.getState().phase).toBe('idle');
    act(() => useExport.getState().setOpen(true));
    expect(within(await screen.findByRole('region', { name: 'Unfinished exports' })).getByText(DISK_FULL)).toBeInTheDocument();
  });

  it('after a row is discarded, focus goes to the next row, then to Export', async () => {
    vi.mocked(exporter.jobs).mockResolvedValue([
      job({ jobId: 'one' }),
      job({ jobId: 'two', projectId: 'p2', projectName: 'Vlog' }),
    ]);
    await openWithJobs(2);
    vi.mocked(exporter.jobs).mockResolvedValue([job({ jobId: 'two', projectId: 'p2', projectName: 'Vlog' })]);
    await userEvent.click(screen.getAllByRole('button', { name: 'Discard' })[0]);
    expect(exporter.discard).toHaveBeenCalledWith('one');
    await waitFor(() => expect(screen.getByRole('button', { name: 'Resume' })).toHaveFocus());
    vi.mocked(exporter.jobs).mockResolvedValue([]);
    await userEvent.keyboard('{Tab}');
    expect(screen.getByRole('button', { name: 'Discard' })).toHaveFocus();
    await userEvent.keyboard('{Enter}');
    expect(exporter.discard).toHaveBeenLastCalledWith('two');
    await waitFor(() => expect(screen.getByRole('button', { name: 'Export 1 video' })).toHaveFocus());
    expect(screen.queryByRole('region', { name: 'Unfinished exports' })).toBeNull();
  });

  it('a Discard Rust refuses is said, and the row stays', async () => {
    vi.mocked(exporter.jobs).mockResolvedValue([job()]);
    vi.mocked(exporter.discard).mockRejectedValueOnce(new Error('The export couldn’t be discarded. Try again.'));
    await openWithJobs(1);
    await userEvent.click(screen.getByRole('button', { name: 'Discard' }));
    await waitFor(() =>
      expect(useToasts.getState().toasts.map((t) => t.text)).toContain('The export couldn’t be discarded. Try again.'),
    );
    expect(screen.getByText('Resume export of Billing tutorial: 3 of 10 parts done')).toBeInTheDocument();
  });

  it('pausing an export running in the background shows its row paused', async () => {
    vi.mocked(exporter.jobs).mockResolvedValue([job({ state: 'running' })]);
    await openWithJobs(1);
    expect(screen.getByText('Exporting Billing tutorial: 3 of 10 parts done')).toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'Pause' }));
    expect(exporter.pause).toHaveBeenCalledWith('old');
    expect(await screen.findByText('Resume export of Billing tutorial: 3 of 10 parts done')).toBeInTheDocument();
    // The keyboard stays in the row: on its Resume.
    await waitFor(() => expect(screen.getByRole('button', { name: 'Resume' })).toHaveFocus());
  });

  it('a job paused by a hardware failure can finish in compatibility mode', async () => {
    vi.mocked(exporter.jobs).mockResolvedValue([
      job({ message: coded('exports.decodeFailed'), compatibility: { partsToRedo: 3, chosen: false } }),
    ]);
    await openWithJobs(1);
    const row = rowOf('Resume export of Billing tutorial: 3 of 10 parts done');
    expect(
      within(row).getByText(
        "A clip couldn't be decoded with hardware. Resume to try again, or finish in compatibility mode.",
      ),
    ).toBeInTheDocument();
    const finish = within(row).getByRole('button', { name: 'Finish in compatibility mode' });
    // Described by its row, so it's clear which export it finishes, then by what it costs.
    expect(finish).toHaveAccessibleDescription(
      /^Resume export of Billing tutorial: 3 of 10 parts done .* Slower\. Redoes 3 finished parts\.$/,
    );
    finish.focus();
    await userEvent.keyboard('{Enter}');
    expect(exporter.resume).toHaveBeenCalledWith('old', expect.any(Function), 'compatible');
  });

  it('a paused job whose person chose compatibility mode offers one resume, which continues in it', async () => {
    vi.mocked(exporter.jobs).mockResolvedValue([job({ compatibility: { partsToRedo: 0, chosen: true } })]);
    await openWithJobs(1);
    const resume = screen.getByRole('button', { name: 'Resume in compatibility mode' });
    const row = resume.closest('li')!;
    expect(within(row).queryByRole('button', { name: 'Finish in compatibility mode' })).toBeNull();
    expect(within(row).queryByRole('button', { name: 'Resume' })).toBeNull();
    await userEvent.click(resume);
    // The job keeps the person's choice: a plain resume continues in compatibility mode.
    expect(exporter.resume).toHaveBeenCalledWith('old', expect.any(Function));
  });

  it('a row says how much disk its finished parts use', async () => {
    vi.mocked(exporter.jobs).mockResolvedValue([job({ bytes: 2_140_000_000 }), job({ jobId: 'new', bytes: 0 })]);
    await openWithJobs(2);
    const [kept, fresh] = within(screen.getByRole('region', { name: 'Unfinished exports' })).getAllByRole('listitem');
    expect(within(kept).getByText('Uses 2.1 GB')).toBeInTheDocument();
    expect(within(kept).getByRole('button', { name: 'Discard' })).toHaveAccessibleDescription(/Uses 2\.1 GB/);
    expect(within(fresh).queryByText(/^Uses /)).toBeNull();
  });

  it('a job another build of Klipolo made offers Start over and Discard, never Resume', async () => {
    vi.mocked(exporter.jobs).mockResolvedValue([
      job({ otherBuild: true, message: coded('exports.diskFull') }),
      job({ jobId: 'vlog', projectId: 'p2', projectName: 'Vlog', otherBuild: true }),
    ]);
    await openWithJobs(2);
    const here = rowOf('Unfinished export of Billing tutorial: 3 of 10 parts done');
    expect(
      within(here).getByText('Another version of Klipolo made this export, so it can’t resume.'),
    ).toBeInTheDocument();
    expect(within(here).queryByRole('button', { name: /^Resume/ })).toBeNull();
    expect(within(here).queryByRole('button', { name: 'Finish in compatibility mode' })).toBeNull();
    expect(within(here).getByRole('button', { name: 'Discard' })).toBeInTheDocument();
    // Another project's: Discard only, from here.
    const other = rowOf('Unfinished export of Vlog: 3 of 10 parts done');
    expect(within(other).getAllByRole('button').map((b) => b.textContent)).toEqual(['Discard']);
    await userEvent.click(within(here).getByRole('button', { name: 'Start over with your latest changes' }));
    await waitFor(() =>
      expect(exporter.start).toHaveBeenCalledWith(['landscape'], 'maximum', expect.any(Function), {
        replaces: 'old',
      }),
    );
  });

  it('an earlier project’s job that can’t be read can only be discarded', async () => {
    vi.mocked(exporter.jobs).mockResolvedValue([
      job({
        projectId: '',
        projectName: '',
        files: [],
        state: 'failed',
        message: coded('exports.unreadable'),
        segmentsDone: 0,
        segmentsTotal: 0,
        stale: true,
      }),
    ]);
    await openWithJobs(1);
    const row = rowOf('An export Klipolo can’t read');
    expect(within(row).getByText('It can only be discarded.')).toBeInTheDocument();
    expect(within(row).queryByText(/parts done/)).toBeNull();
    expect(within(row).queryByRole('button', { name: 'Resume' })).toBeNull();
    expect(within(row).getByRole('button', { name: 'Discard' })).toBeInTheDocument();
  });

  it('a kept job whose project changed says so in the paused view, with Start over beside Resume', async () => {
    await openAndExport();
    vi.mocked(exporter.jobs).mockResolvedValue([job({ jobId: 'job', stale: true })]);
    emit({ type: 'paused', message: coded('exports.encoderBusy'), compatibility: null });
    expect(await screen.findByText('This project changed since the export started.')).toBeInTheDocument();
    expect(screen.getByText('Resume finishes the version from when you started.')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Start over with your latest changes' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Resume' })).toHaveFocus();
  });
});
