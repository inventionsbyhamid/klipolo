import { act, renderHook, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { createRef, Profiler } from 'react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { MediaJobInfo } from '../../bindings/MediaJobInfo';
import type { MediaJobList } from '../../bindings/MediaJobList';
import { renderWithProviders } from '../../test/render';

vi.mock('../../bridge/mediaJobs', () => ({
  mediaJobs: {
    list: vi.fn(),
    pause: vi.fn(),
    resume: vi.fn(),
    stop: vi.fn(),
    retry: vi.fn(),
    setAuto: vi.fn(),
    onChange: vi.fn(),
  },
}));
vi.mock('../../bridge/engine', () => ({
  engine: {
    attach: vi.fn(() =>
      Promise.resolve({
        port: 0,
        token: '',
        maxPreviewPixels: 921_600,
        adapter: 'Mock',
        presenter: 'socket',
        gpuClass: 'integrated',
      }),
    ),
    setVisible: vi.fn(() => Promise.resolve()),
    seek: vi.fn(() => Promise.resolve()),
    pause: vi.fn(() => Promise.resolve(0)),
  },
}));
const { mediaJobs } = await import('../../bridge/mediaJobs');
const { useMediaJobs, PROGRESS_DRAW_MS } = await import('../../state/mediaJobs');
const { useUi } = await import('../../state/ui');
const { useToasts } = await import('../../state/toasts');
const { MediaJobsPill } = await import('./MediaJobsPill');
const { MediaJobsFeed } = await import('./MediaJobsFeed');
const { SETTLE_MS } = await import('./usePressGuard');
const { TopBar } = await import('../../layout/TopBar');
const { CopiesSettings } = await import('./CopiesSettings');
const { useProject } = await import('../../state/project');
const { useEngineSession } = await import('../preview/useEngineSession');
const { fixtureSnapshot } = await import('../../test/fixtures');

const working: MediaJobInfo = {
  path: '/v/intro.mp4',
  name: 'intro.mp4',
  state: 'working',
  progress: 0.4,
  why: 'tooLarge',
  reason: null,
};
/** A reason as Rust says it: a code of `appCodes.json` and its values. */
const coded = (code: string, params: Record<string, string> = {}) => ({ code, params });
const paused: MediaJobInfo = {
  path: '/v/b.mp4',
  name: 'b.mp4',
  state: 'paused',
  progress: 0.1,
  why: 'tooLarge',
  reason: coded('copies.paused.diskFull'),
};
/** Rust's refusals (codes): a file the open project doesn't have; the driver gone; a command answered late. */
const NOT_IN_PROJECT = 'copies.refused.notInProject';
const UNAVAILABLE = 'copies.refused.unavailable';
const SLOW_DRIVE = 'copies.refused.slowDrive';
/** As `call` throws it: the coded error is the cause. */
const refusal = (code: string) => new Error('', { cause: coded(code) });
const failed = (name: string): MediaJobInfo => ({
  path: `/v/${name}`,
  name,
  state: 'failed',
  progress: 0,
  why: 'tooLarge',
  reason: coded('copies.failed.other', { name }),
});
/** The pill alone (the top bar gives it Settings to hand the focus to). */
const pill = () => <MediaJobsPill focusAfter={createRef()} />;

beforeEach(() => {
  vi.clearAllMocks();
  for (const command of ['list', 'pause', 'resume', 'stop', 'retry', 'setAuto'] as const)
    vi.mocked(mediaJobs[command]).mockResolvedValue(undefined as never);
  vi.mocked(mediaJobs.onChange).mockResolvedValue(() => undefined);
  useMediaJobs.setState({ jobs: [], dismissed: [], pausedByPerson: false, pausedByAction: false });
  useUi.setState({ autoCopies: true });
  useToasts.setState({ toasts: [] });
  useProject.setState({ snapshot: fixtureSnapshot(), pending: 0, switching: false });
});

afterEach(() => vi.useRealTimers());

describe('the editing copies pill', () => {
  it('stays out of the way until there is work, then says how much', () => {
    useMediaJobs.setState({ jobs: [] });
    const { rerender } = renderWithProviders(pill());
    expect(screen.queryByRole('button', { name: /Preparing/ })).toBeNull();
    act(() => useMediaJobs.setState({ jobs: [working, paused] }));
    rerender(pill());
    // The percentage is formatted for the language shown (Intl: "25%" in English).
    expect(screen.getByRole('button', { name: 'Preparing 2 files, 25%' })).toBeInTheDocument();
    act(() => useMediaJobs.setState({ jobs: [working] }));
    rerender(pill());
    expect(screen.getByRole('button', { name: 'Preparing 1 file, 40%' })).toBeInTheDocument();
    // Finished copies, and waiting ones while automatic copies are off (nothing would start them), aren't counted.
    act(() => useMediaJobs.setState({ jobs: [{ ...working, state: 'done', progress: 1 }] }));
    expect(screen.queryByRole('button', { name: /Preparing/ })).toBeNull();
    act(() => {
      useUi.setState({ autoCopies: false });
      useMediaJobs.setState({ jobs: [{ ...working, state: 'waiting', reason: null }] });
    });
    expect(screen.queryByRole('button', { name: /Preparing/ })).toBeNull();
  });

  it('lists each file with its reason and offers pause, resume, stop and the automatic switch', async () => {
    useMediaJobs.setState({ jobs: [working, paused] });
    renderWithProviders(pill());
    await userEvent.click(screen.getByRole('button', { name: /Preparing/ }));
    expect(
      screen.getByText('Paused because the disk is almost full. Free up space, then press Resume all.'),
    ).toBeInTheDocument();
    expect(screen.getByRole('progressbar', { name: 'intro.mp4' })).toHaveAttribute('aria-valuenow', '40');
    vi.useFakeTimers({ toFake: ['Date'] });
    await userEvent.click(screen.getByRole('button', { name: 'Pause all' }));
    // The person's own pause: an action's (another app's) is separate, and neither undoes the other.
    expect(mediaJobs.pause).toHaveBeenCalledWith('person');
    // The same button now resumes: a press at once is the tail of a double click, and does nothing.
    const resume = await screen.findByRole('button', { name: 'Resume all' });
    await userEvent.click(resume);
    expect(mediaJobs.resume).not.toHaveBeenCalled();
    vi.setSystemTime(Date.now() + SETTLE_MS);
    await userEvent.click(resume);
    expect(mediaJobs.resume).toHaveBeenCalledWith('person');
    await userEvent.click(screen.getByRole('button', { name: 'Stop making an editing copy of intro.mp4' }));
    expect(mediaJobs.stop).toHaveBeenCalledWith('/v/intro.mp4');
    await userEvent.click(screen.getByRole('switch', { name: 'Make editing copies automatically' }));
    expect(mediaJobs.setAuto).toHaveBeenCalledWith(false);
    expect(useUi.getState().autoCopies).toBe(false);
  });

  it('words each of Rust’s reasons, offers Try again once a copy failed or was stopped, and says a refusal', async () => {
    useMediaJobs.setState({
      jobs: [
        { ...working, path: '/v/a.mp4', name: 'a.mp4', state: 'waiting', progress: 0, reason: null },
        { ...paused, reason: coded('copies.paused.playing') },
        {
          ...working,
          path: '/v/c.mp4',
          name: 'c.mp4',
          state: 'failed',
          reason: coded('copies.failed.other', { name: 'c.mp4' }),
        },
        {
          ...working,
          path: '/v/d.mp4',
          name: 'd.mp4',
          state: 'failed',
          reason: coded('copies.failed.missing', { name: 'd.mp4' }),
        },
        { ...working, path: '/v/e.mp4', name: 'e.mp4', state: 'stopped', progress: 0 },
        { ...working, path: '/v/f.mp4', name: 'f.mp4', state: 'done', progress: 1 },
        {
          ...working,
          path: '/v/g.mp4',
          name: 'g.mp4',
          state: 'failed',
          why: null,
          reason: coded('copies.failed.unreadable', { name: 'g.mp4' }),
        },
        {
          ...working,
          path: '/v/h.mp4',
          name: 'h.mp4',
          state: 'failed',
          reason: coded('copies.failed.cacheUnwritable'),
        },
        // A code this build doesn't know is said by its state, never shown raw.
        { ...working, path: '/v/i.mp4', name: 'i.mp4', state: 'failed', reason: coded('copies.failed.new') },
      ],
    });
    renderWithProviders(pill());
    await userEvent.click(screen.getByRole('button', { name: 'Preparing 2 files, 5%' }));
    for (const text of [
      'Waiting its turn',
      'Paused while the video plays. It goes on when you stop.',
      "Klipolo couldn't make an editing copy of c.mp4. Press Try again.",
      "d.mp4 isn't where it was. Put it back, then open the project again.",
      'Stopped. Press Try again to make it.',
      "g.mp4 can't be read as video, so it has no editing copy. The preview plays the file itself.",
      "Klipolo can't save editing copies on this disk. Check that it isn't full or read-only, then press Try again.",
      "Klipolo couldn't make an editing copy of i.mp4. Press Try again.",
    ])
      expect(screen.getByText(text)).toBeInTheDocument();
    // Why each file still to be copied gets one (Rust's `why`): the waiting and the paused one, not the failed ones.
    expect(screen.getAllByText("It's larger than this Mac plays smoothly.")).toHaveLength(2);
    // A finished copy isn't listed; paused only while playing, it goes on by itself: Pause all, no Resume all.
    expect(screen.queryByText('f.mp4')).toBeNull();
    expect(screen.getByRole('button', { name: 'Pause all' })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Resume all' })).toBeNull();
    await userEvent.click(screen.getByRole('button', { name: 'Try again to make an editing copy of e.mp4' }));
    expect(mediaJobs.retry).toHaveBeenCalledWith('/v/e.mp4');
    vi.mocked(mediaJobs.retry).mockRejectedValueOnce(refusal(NOT_IN_PROJECT));
    await userEvent.click(screen.getByRole('button', { name: 'Try again to make an editing copy of c.mp4' }));
    await waitFor(() =>
      expect(useToasts.getState().toasts.map((toast) => toast.text)).toEqual([
        "That file isn't in the open project, so its editing copy didn't change.",
      ]),
    );
  });

  it('says a stopped copy maker is trying again, then offers Try again once it keeps stopping', async () => {
    useMediaJobs.setState({
      jobs: [
        { ...paused, reason: coded('copies.paused.workerStopped') },
        { ...failed('k.mp4'), reason: coded('copies.failed.workerKeepsStopping') },
      ],
    });
    renderWithProviders(pill());
    await userEvent.click(screen.getByRole('button', { name: /^Preparing 1 file/ }));
    expect(screen.getByText('Making the copy stopped unexpectedly. Trying again.')).toBeInTheDocument();
    expect(screen.getByText('Making the copy keeps stopping. Press Try again later.')).toBeInTheDocument();
    // Never the file's fault; it starts again by itself, so it isn't waiting for the person.
    expect(screen.queryByText(/couldn't make an editing copy/)).toBeNull();
    expect(
      screen.queryByRole('button', { name: `Try again to make an editing copy of ${paused.name}` }),
    ).toBeNull();
    await userEvent.click(screen.getByRole('button', { name: 'Try again to make an editing copy of k.mp4' }));
    expect(mediaJobs.retry).toHaveBeenCalledWith('/v/k.mp4');
  });

  it('says a copy waits while a take records, and that it goes on by itself after it', async () => {
    useMediaJobs.setState({ jobs: [{ ...paused, reason: coded('copies.paused.recording') }] });
    renderWithProviders(pill());
    await userEvent.click(screen.getByRole('button', { name: /^Preparing 1 file/ }));
    expect(screen.getByText('Paused while you record. It goes on when the take ends.')).toBeInTheDocument();
    // Nothing for the person to resume: Pause all can still hold it off.
    expect(screen.getByRole('button', { name: 'Pause all' })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Resume all' })).toBeNull();
  });

  it('says a copy waits while Klipolo analyzes a video, and that it goes on by itself after', async () => {
    useMediaJobs.setState({ jobs: [{ ...paused, reason: coded('copies.paused.analyzing') }] });
    renderWithProviders(pill());
    await userEvent.click(screen.getByRole('button', { name: /^Preparing 1 file/ }));
    expect(screen.getByText('Paused while Klipolo analyzes your video. It goes on by itself.')).toBeInTheDocument();
    // Nothing for the person to resume: Pause all can still hold it off.
    expect(screen.getByRole('button', { name: 'Pause all' })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Resume all' })).toBeNull();
  });

  it('can be turned back on in Settings once turned off in the popover and the pill is gone', async () => {
    useMediaJobs.setState({ jobs: [working] });
    renderWithProviders(<TopBar />);
    await userEvent.click(screen.getByRole('button', { name: /^Preparing 1 file/ }));
    await userEvent.click(screen.getByRole('switch', { name: 'Make editing copies automatically' }));
    expect(mediaJobs.setAuto).toHaveBeenLastCalledWith(false);
    // Rust forgets the copy being made and lists none while copies are off: closed, the pill goes.
    act(() => useMediaJobs.getState().receive([], []));
    await userEvent.keyboard('{Escape}');
    expect(screen.queryByRole('button', { name: /Preparing|Editing copies/ })).toBeNull();

    renderWithProviders(<CopiesSettings />);
    const again = screen.getByRole('switch', { name: 'Make editing copies automatically' });
    expect(again).toHaveAttribute('aria-checked', 'false');
    await userEvent.click(again);
    expect(mediaJobs.setAuto).toHaveBeenLastCalledWith(true);
    expect(again).toHaveAttribute('aria-checked', 'true');
    expect(localStorage.getItem('klipolo.autoCopies')).toBe('on');
  });

  it('sits in the top bar only while a project is open, and says what copies are for', async () => {
    useMediaJobs.setState({ jobs: [working] });
    const { unmount } = renderWithProviders(<TopBar />);
    const pillButton = screen.getByRole('button', { name: 'Preparing 1 file, 40%' });
    expect(pillButton.closest('.topbar-right')).not.toBeNull();
    await userEvent.click(pillButton);
    const popover = screen.getByRole('dialog', { name: 'Editing copies' });
    expect(popover).toHaveTextContent(
      'Klipolo makes lighter copies of videos that would play slowly on this Mac, so the preview plays smoothly. Exports always use your full-quality files.',
    );
    expect(popover).toHaveTextContent("It's larger than this Mac plays smoothly.");
    unmount();
    // The start screen: no project, no pill.
    act(() => useProject.setState({ snapshot: null }));
    renderWithProviders(<TopBar />);
    expect(screen.queryByRole('button', { name: /Preparing/ })).toBeNull();
  });

  it('stays open when the last copy stops, so Try again is still at hand', async () => {
    useMediaJobs.setState({ jobs: [working] });
    renderWithProviders(pill());
    await userEvent.click(screen.getByRole('button', { name: /Preparing/ }));
    act(() => useMediaJobs.setState({ jobs: [{ ...working, state: 'stopped', progress: 0 }] }));
    expect(
      screen.getByRole('button', { name: 'Try again to make an editing copy of intro.mp4' }),
    ).toBeVisible();
    await userEvent.keyboard('{Escape}');
    expect(screen.queryByRole('button', { name: /Editing copies|Preparing/ })).toBeNull();
  });

  it('keeps a quiet pill for a failed copy until it is tried again or dismissed; a stopped one never keeps it', async () => {
    const copy = failed('intro.mp4');
    useMediaJobs.setState({ jobs: [copy] });
    renderWithProviders(pill());
    await userEvent.click(screen.getByRole('button', { name: 'Copy failed' }));
    await userEvent.click(
      screen.getByRole('button', { name: 'Try again to make an editing copy of intro.mp4' }),
    );
    expect(mediaJobs.retry).toHaveBeenCalledWith('/v/intro.mp4');
    // Dismissed, it leaves the list, and the pill once the popover closes.
    await userEvent.click(screen.getByRole('button', { name: 'Dismiss the failed copy of intro.mp4' }));
    expect(screen.queryByText('intro.mp4')).toBeNull();
    await userEvent.keyboard('{Escape}');
    expect(screen.queryByRole('button', { name: /Copy failed|Editing copies/ })).toBeNull();
    // Tried again, it can fail anew and show again.
    act(() => useMediaJobs.getState().receive([{ ...copy, state: 'waiting', reason: null }]));
    act(() => useMediaJobs.getState().receive([copy]));
    expect(screen.getByRole('button', { name: 'Copy failed' })).toBeInTheDocument();
    // Not while copies are off: Rust still decides files (an unreadable one fails), but nothing would copy it.
    act(() => useUi.setState({ autoCopies: false }));
    expect(screen.queryByRole('button', { name: 'Copy failed' })).toBeNull();
    act(() => useUi.setState({ autoCopies: true }));
    // A copy the person stopped never keeps the pill: that was their choice.
    act(() => useMediaJobs.getState().receive([{ ...working, state: 'stopped', progress: 0 }]));
    expect(screen.queryByRole('button', { name: /Copy failed|Preparing|Editing copies/ })).toBeNull();
  });

  it('offers Resume all only for what the person can resume, never a copy only another app’s pause holds', async () => {
    useMediaJobs.setState({
      jobs: [{ ...paused, reason: coded('copies.paused.held') }],
      pausedByAction: true,
    });
    renderWithProviders(pill());
    await userEvent.click(screen.getByRole('button', { name: /Preparing/ }));
    expect(screen.getByText('Another app paused editing copies. They go on when that app resumes them.')).toBeInTheDocument();
    expect(screen.getByText('Paused by another app')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Resume all' })).toBeNull();
    // Paused for heat (tried again by itself) while others wait: Pause all can still hold the retry off.
    act(() =>
      useMediaJobs.setState({
        jobs: [
          { ...paused, reason: coded('copies.paused.hot') },
          { ...working, state: 'waiting', reason: null },
        ],
        pausedByAction: false,
      }),
    );
    expect(screen.getByRole('button', { name: 'Pause all' })).toBeInTheDocument();
  });

  it('says who paused copies from Rust’s list, so a reload still knows; Resume all lifts only the person’s pause', async () => {
    const held = { ...paused, reason: coded('copies.paused.held') };
    const waiting: MediaJobInfo = {
      ...working,
      path: '/v/w.mp4',
      name: 'w.mp4',
      state: 'waiting',
      reason: null,
    };
    // A window loaded again: nothing pressed here, Rust's list says the person paused them.
    act(() => useMediaJobs.getState().receive([held, waiting], ['person']));
    renderWithProviders(pill());
    await userEvent.click(screen.getByRole('button', { name: /Preparing/ }));
    expect(screen.getAllByText('Paused by you')).toHaveLength(2);
    await userEvent.click(screen.getByRole('button', { name: 'Resume all' }));
    expect(mediaJobs.resume).toHaveBeenCalledWith('person');
    // Rust says the person's pause is lifted and another app's holds: one Resume all can't lift.
    act(() => useMediaJobs.getState().receive([held, waiting], ['action']));
    expect(screen.getAllByText('Paused by another app')).toHaveLength(2);
    expect(screen.getByText('Another app paused editing copies. They go on when that app resumes them.')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Resume all' })).toBeNull();
  });

  it('takes the second click of a double click on Stop as Stop’s, never as Try again', async () => {
    vi.useFakeTimers({ toFake: ['Date'] });
    useMediaJobs.setState({ jobs: [working] });
    renderWithProviders(pill());
    await userEvent.click(screen.getByRole('button', { name: /Preparing/ }));
    const action = screen.getByRole('button', { name: 'Stop making an editing copy of intro.mp4' });
    await userEvent.click(action);
    // Rust answers at once: the same button now says Try again.
    act(() => useMediaJobs.getState().receive([{ ...working, state: 'stopped', progress: 0 }]));
    await userEvent.click(action);
    expect(mediaJobs.retry).not.toHaveBeenCalled();
    vi.setSystemTime(Date.now() + SETTLE_MS);
    await userEvent.click(action);
    expect(mediaJobs.retry).toHaveBeenCalledWith('/v/intro.mp4');
  });
  it('draws progress at most twice a second, and never the top bar for it; the popover is only there while open', () => {
    vi.useFakeTimers();
    let commits = 0;
    renderWithProviders(
      <Profiler id="topbar" onRender={() => commits++}>
        <TopBar />
      </Profiler>,
    );
    const at = (progress: number): MediaJobInfo[] => [{ ...working, progress }];
    // A new row is drawn at once.
    act(() => useMediaJobs.getState().receive(at(0.1)));
    expect(screen.getByRole('button', { name: 'Preparing 1 file, 10%' })).toBeInTheDocument();
    commits = 0;
    // A second of progress, a percent every 50 ms (five times Rust's rate): drawn once a throttle window.
    for (let i = 1; i <= 20; i++)
      act(() => {
        useMediaJobs.getState().receive(at(0.1 + i / 100));
        vi.advanceTimersByTime(50);
      });
    expect(commits).toBeLessThanOrEqual(2);
    act(() => vi.advanceTimersByTime(PROGRESS_DRAW_MS));
    expect(screen.getByRole('button', { name: 'Preparing 1 file, 30%' })).toBeInTheDocument();
    // Progress under a whole percent draws nothing, even once its window comes.
    commits = 0;
    act(() => {
      useMediaJobs.getState().receive(at(0.304));
      vi.advanceTimersByTime(PROGRESS_DRAW_MS);
    });
    expect(commits).toBe(0);
    // A change of state is drawn at once (it finished: the pill goes).
    act(() => useMediaJobs.getState().receive([{ ...working, state: 'done', progress: 1 }]));
    expect(screen.queryByRole('button', { name: /Preparing/ })).toBeNull();
    // Closed, the popover isn't mounted, so nothing in it follows the list.
    expect(screen.queryByRole('dialog')).toBeNull();
  });
});

describe('the keyboard focus around editing copies', () => {
  const focused = () => document.activeElement as HTMLElement;

  it('stays on the footer’s button and a row’s button as Rust’s list changes them', async () => {
    useMediaJobs.setState({ jobs: [working] });
    renderWithProviders(<TopBar />);
    await userEvent.click(screen.getByRole('button', { name: /Preparing/ }));
    act(() => screen.getByRole('button', { name: 'Pause all' }).focus());
    await userEvent.keyboard(' ');
    expect(mediaJobs.pause).toHaveBeenCalledWith('person');
    await waitFor(() => expect(focused()).toHaveAccessibleName('Resume all'));
    expect(screen.getByRole('dialog')).toContainElement(focused());
    act(() => screen.getByRole('button', { name: 'Stop making an editing copy of intro.mp4' }).focus());
    await userEvent.keyboard('{Enter}');
    expect(mediaJobs.stop).toHaveBeenCalledWith('/v/intro.mp4');
    act(() => useMediaJobs.getState().receive([{ ...working, state: 'stopped', progress: 0 }]));
    expect(focused()).toHaveAccessibleName('Try again to make an editing copy of intro.mp4');
  });

  it('moves on to the next row, then the switch, then Settings as what had it goes; never to the page', async () => {
    useMediaJobs.setState({ jobs: [failed('a.mp4'), failed('b.mp4')] });
    renderWithProviders(<TopBar />);
    await userEvent.click(screen.getByRole('button', { name: '2 copies failed' }));
    act(() => screen.getByRole('button', { name: 'Dismiss the failed copy of a.mp4' }).focus());
    await userEvent.keyboard('{Enter}');
    expect(focused()).toHaveAccessibleName('Try again to make an editing copy of b.mp4');
    act(() => screen.getByRole('button', { name: 'Dismiss the failed copy of b.mp4' }).focus());
    await userEvent.keyboard('{Enter}');
    // The list is empty: the switch.
    expect(focused()).toHaveAccessibleName('Make editing copies automatically');
    // Closed with nothing left to show, the pill goes: the top bar's next control.
    await userEvent.keyboard('{Escape}');
    expect(screen.queryByRole('button', { name: /Editing copies/ })).toBeNull();
    await waitFor(() => expect(focused()).toHaveAccessibleName('Settings'));
    // The pill itself focused (closed) when its last copy finishes: Settings too.
    act(() => useMediaJobs.getState().receive([working]));
    act(() => screen.getByRole('button', { name: /Preparing/ }).focus());
    act(() => useMediaJobs.getState().receive([{ ...working, state: 'done', progress: 1 }]));
    expect(focused()).toHaveAccessibleName('Settings');
  });
});

describe('the editing copies store', () => {
  it('lists on opening, follows each event, drops a listing older than an event, and empties when closed', async () => {
    let heard: ((list: MediaJobList) => void) | undefined;
    vi.mocked(mediaJobs.onChange).mockImplementation((handler) => {
      heard = handler;
      return Promise.resolve(() => undefined);
    });
    let answer: (list: MediaJobList) => void = () => undefined;
    vi.mocked(mediaJobs.list).mockReturnValue(new Promise((resolve) => (answer = resolve)));
    const stop = useMediaJobs.getState().listen();
    const loading = useMediaJobs.getState().load();
    await waitFor(() => expect(heard).toBeDefined());
    // Who holds copies comes with the list, so a window loaded again knows the person paused them.
    heard!({ jobs: [working], held: ['person'] });
    answer({ jobs: [], held: [] });
    await loading;
    expect(useMediaJobs.getState()).toMatchObject({
      jobs: [working],
      pausedByPerson: true,
      pausedByAction: false,
    });
    // Without Tauri the listing fails, and the list stays as it is.
    vi.mocked(mediaJobs.list).mockRejectedValue(new Error('no Tauri'));
    await useMediaJobs.getState().load();
    expect(useMediaJobs.getState().jobs).toEqual([working]);
    stop();
    expect(useMediaJobs.getState().jobs).toEqual([]);
  });

  it('turns the switch back when Rust can’t take it, and takes a pause Rust answered late as done', async () => {
    const copies = useMediaJobs.getState();
    vi.mocked(mediaJobs.setAuto).mockRejectedValueOnce(refusal(UNAVAILABLE));
    expect(await copies.setAuto(false)).toBe("Editing copies aren't available right now. Try again in a moment.");
    expect(useUi.getState().autoCopies).toBe(true);
    // A slow drive: the command is still applied once it answers.
    vi.mocked(mediaJobs.pause).mockRejectedValueOnce(refusal(SLOW_DRIVE));
    expect(await copies.pause('person')).toBeNull();
    expect(useMediaJobs.getState().pausedByPerson).toBe(true);
  });

  it('keeps the Auto copies switch on this Mac', async () => {
    const copies = useMediaJobs.getState();
    expect(await copies.setAuto(false)).toBeNull();
    expect(mediaJobs.setAuto).toHaveBeenCalledWith(false);
    expect(localStorage.getItem('klipolo.autoCopies')).toBe('off');
    expect(await copies.setAuto(true)).toBeNull();
    expect(localStorage.getItem('klipolo.autoCopies')).toBe('on');
  });

  it('never lets progress held back undo a pause answered meanwhile', async () => {
    vi.useFakeTimers();
    const copies = useMediaJobs.getState();
    copies.receive([working], []);
    // Only progress moved: held back for the throttle, with the holds of its time (none).
    copies.receive([{ ...working, progress: 0.5 }], []);
    expect(await copies.pause('person')).toBeNull();
    vi.advanceTimersByTime(PROGRESS_DRAW_MS);
    expect(useMediaJobs.getState()).toMatchObject({
      jobs: [{ ...working, progress: 0.5 }],
      pausedByPerson: true,
    });
  });

  it('sends the person’s setting as the editor opens and once the engine attaches; a failure is never unhandled', async () => {
    useUi.setState({ autoCopies: false });
    vi.mocked(mediaJobs.setAuto).mockRejectedValue(new Error('no Tauri'));
    // The editor opening: before anything is listed, and before the engine can attach.
    const feed = renderWithProviders(<MediaJobsFeed />);
    expect(mediaJobs.setAuto).toHaveBeenCalledWith(false);
    expect(vi.mocked(mediaJobs.setAuto).mock.invocationCallOrder[0]).toBeLessThan(
      vi.mocked(mediaJobs.list).mock.invocationCallOrder[0],
    );
    feed.unmount();
    vi.mocked(mediaJobs.setAuto).mockClear();
    const { unmount } = renderHook(() => useEngineSession());
    await waitFor(() => expect(mediaJobs.setAuto).toHaveBeenCalledWith(false));
    unmount();
  });

  it('the preview says a file it needs is missing in the words of the copy’s row', async () => {
    const { engine } = await import('../../bridge/engine');
    vi.mocked(engine.attach).mockClear();
    const { unmount } = renderHook(() => useEngineSession());
    await waitFor(() => expect(engine.attach).toHaveBeenCalled());
    const onEvent = vi.mocked(engine.attach).mock.calls[0][0];
    act(() => onEvent({ type: 'notice', notice: coded('copies.failed.missing', { name: 'intro.mp4' }) }));
    expect(useToasts.getState().toasts.map((toast) => toast.text)).toEqual([
      "intro.mp4 isn't where it was. Put it back, then open the project again.",
    ]);
    unmount();
  });
});
