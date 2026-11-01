import { act, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { ExportEvent } from '../../bindings/ExportEvent';
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
    }),
);
vi.mock('../../bridge/exporter', () => ({
  exporter: {
    plan: vi.fn(planned),
    start: vi.fn(() => Promise.resolve()),
    cancel: vi.fn(() => Promise.resolve()),
    reveal: vi.fn(() => Promise.resolve()),
    open: vi.fn(() => Promise.resolve()),
    files: vi.fn(() => Promise.resolve([{ name: 'Billing tutorial 16x9 2.mp4', bytes: 148_000_000 }])),
    pickFolder: vi.fn(() => Promise.resolve('Desktop / Clips')),
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
const alert = () => screen.getByRole('alert');
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

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(exporter.plan).mockImplementation(planned as typeof exporter.plan);
  useProject.setState({ snapshot: fixtureSnapshot(), pending: 0, apply });
  useExport.getState().reset();
  useExport.setState({ open: false, quality: 'high', folder: null });
  useAnalysis.setState({ states: {} });
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
    expect(await screen.findByText('2 videos, about 280 MB')).toBeInTheDocument();
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
    expect(await screen.findByText('1 video, about 140 MB')).toBeInTheDocument();
    await userEvent.click(screen.getByRole('radio', { name: 'Smaller file' }));
    expect(await screen.findByText('1 video, about 80 MB')).toBeInTheDocument();
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

  it('a full disk says what wasn’t saved, keeps the files in place, and Try again exports the rest', async () => {
    renderExport();
    await userEvent.click(screen.getByRole('button', { name: 'Export' }));
    await userEvent.click(screen.getByRole('checkbox', { name: /^9:16/ }));
    await userEvent.click(await screen.findByRole('button', { name: 'Export 2 videos' }));
    emit(
      { ...started, files: [landscape, portrait] } as ExportEvent,
      progress(100, 0),
      { type: 'fileReady', index: 0, path: landscape.path },
      { type: 'failed', message: { code: 'exports.diskFull', params: {} } },
    );
    expect(await screen.findByText('9:16 was not saved. The disk is full. Free up space, then try again.', { selector: 'span' })).toBeInTheDocument();
    expect(alert()).toHaveTextContent('9:16 was not saved. The disk is full. Free up space, then try again.');
    expect(status()).toBeEmptyDOMElement();
    expect(screen.getByText('Billing tutorial 16x9.mp4')).toBeInTheDocument();
    const again = screen.getByRole('button', { name: 'Try again' });
    expect(again).toHaveFocus();
    await userEvent.click(again);
    await waitFor(() => expect(exporter.start).toHaveBeenCalledTimes(2));
    expect(exporter.start).toHaveBeenLastCalledWith(['portrait'], 'high', expect.any(Function));
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
