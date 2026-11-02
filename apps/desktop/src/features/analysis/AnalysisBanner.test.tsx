import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { act, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Asset } from '../../bindings/Asset';
import type { Estimate } from '../../bindings/Estimate';
import type { Item } from '../../bindings/Item';
import type { Snapshot } from '../../bindings/Snapshot';
import type { SourceRole } from '../../bindings/SourceRole';
import { renderWithProviders } from '../../test/render';

vi.mock('../../bridge/analysis', () => ({
  analysisBridge: {
    estimate: vi.fn(),
    start: vi.fn(),
    cancel: vi.fn(),
    states: vi.fn(),
    onEvent: vi.fn(),
  },
}));
const { analysisBridge } = await import('../../bridge/analysis');
const { offerAgain, useAnalysis } = await import('../../state/analysis');
const { useAi } = await import('../../ai/aiStatus');
const { useProject } = await import('../../state/project');
const { AnalysisBanner } = await import('./AnalysisBanner');
const { RoughCaptionsNote } = await import('./RoughCaptionsNote');

const bridge = vi.mocked(analysisBridge);

function clip(id: string, name: string, kind: Asset['kind'] = 'video'): Asset {
  return {
    id,
    kind,
    path: `/clips/${name}`,
    name,
    durationUs: 65_000_000,
    width: 1920,
    height: 1080,
    fps: 30,
    hasAudio: true,
  };
}

const CLIPS = [clip('a', 'rosa-interview-a.mov'), clip('b', 'rosa-wheel-broll.mov')];
/** What a clip whose file is gone says. */
const GONE = "Klipolo can't find this clip's file. Put it back or import it again.";

/** A project of `assets`, each played by a clip on the main track (what analysis is for); `layers` on its tracks. */
function showProject(assets: Asset[], { layers = [] as Item[], extra = [] as Asset[] } = {}) {
  const main = assets.map((a) => ({ id: `clip-${a.id}`, source: { type: 'asset', id: a.id } }));
  const tracks = [{ id: 'audio', kind: 'audio', items: layers }];
  const snapshot = {
    project: { assets: [...assets, ...extra], recordings: [], timeline: { main, tracks } },
  } as unknown as Snapshot;
  useProject.setState({ snapshot });
}

/** A sound layer of `assetId`: music when it ducks, a sound effect when it doesn't. */
function soundLayer(id: string, assetId: string, duck?: number): Item {
  return {
    id,
    startUs: 0,
    durationUs: 16_000_000,
    attach: null,
    layout: { anchor: 'topLeft', rect: { x: 0, y: 0, w: 1, h: 1 }, rotation: 0, opacity: 1, radius: 0, overrides: {} },
    content: { type: 'audio', assetId, sourceInUs: 0, volume: 0.3, fadeInUs: 0, fadeOutUs: 0, effect: null, duck },
    effects: [],
  };
}

function offer(over: Partial<Estimate> = {}): Estimate {
  return { usd: 0.04, seconds: 130, cached: [], clips: ['a', 'b'], missing: [], ...over };
}

beforeEach(() => {
  vi.clearAllMocks();
  bridge.start.mockResolvedValue(undefined);
  bridge.cancel.mockResolvedValue(undefined);
  useAnalysis.setState({ states: {}, estimate: null, error: null, dismissed: [], busy: false });
  useAi.setState({ status: { mode: 'relay', budgetUsd: 2, spentUsd: 0, active: true }, settingsOpen: false });
});

describe('the analysis banner', () => {
  it('offers the clips with their cost and the credit left, and Analyze starts them', async () => {
    bridge.estimate.mockResolvedValue(offer({ creditLeftUsd: 2 }));
    showProject(CLIPS);
    renderWithProviders(<AnalysisBanner />);

    expect(await screen.findByText('Analyze 2 clips for about $0.04')).toBeInTheDocument();
    expect(screen.getByText(/You have \$2\.00 left on your access code\./)).toBeInTheDocument();
    expect(bridge.estimate).toHaveBeenCalledWith(['a', 'b']);
    await userEvent.click(screen.getByRole('button', { name: 'Analyze' }));
    expect(bridge.start).toHaveBeenCalledWith(['a', 'b']);
  });

  it('offers only what main-track clips play: not the pack’s music, a placed sound or a song never placed', async () => {
    bridge.estimate.mockResolvedValue(offer({ clips: ['a'] }));
    const music = [
      clip('klipolo-pack-soft-ambient', 'Soft ambient', 'audio'),
      clip('klipolo-pack-whoosh', 'Whoosh', 'audio'),
      clip('song', 'my-song.mp3', 'audio'),
      clip('logo', 'logo.png', 'image'),
    ];
    showProject([CLIPS[0]], {
      extra: music,
      layers: [soundLayer('bed', 'klipolo-pack-soft-ambient', -12), soundLayer('whoosh', 'klipolo-pack-whoosh')],
    });
    renderWithProviders(<AnalysisBanner />);

    expect(await screen.findByText('Analyze 1 clip for about $0.04')).toBeInTheDocument();
    expect(bridge.estimate).toHaveBeenCalledTimes(1);
    expect(bridge.estimate).toHaveBeenCalledWith(['a']);
  });

  it('refuses with why when the estimate is over the credit left', async () => {
    const refusal =
      'Your access code has $0.02 left, and this needs about $0.07. Analyze fewer or shorter files, or add your own ' +
      'Meta key in Settings.';
    bridge.estimate.mockResolvedValue(offer({ creditLeftUsd: 0.02, refusal }));
    showProject(CLIPS);
    renderWithProviders(<AnalysisBanner />);

    expect(await screen.findByText(new RegExp(refusal.replace(/[.$]/g, '\\$&')))).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Analyze' })).toBeDisabled();
  });

  it('asks for a key or code without one', async () => {
    useAi.setState({ status: { mode: 'none' } });
    bridge.estimate.mockResolvedValue(offer());
    showProject(CLIPS);
    renderWithProviders(<AnalysisBanner />);

    expect(await screen.findByText('Add your Meta API key or access code in Settings')).toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'Open Settings' }));
    expect(useAi.getState().settingsOpen).toBe(true);
  });

  it('says why Analyze could not start, and Retry asks for the estimate again before anything starts', async () => {
    bridge.estimate.mockResolvedValue(offer());
    bridge.start.mockRejectedValueOnce(new Error("Klipolo's server is busy right now. Try again in a moment."));
    showProject(CLIPS);
    renderWithProviders(<AnalysisBanner />);
    await userEvent.click(await screen.findByRole('button', { name: 'Analyze' }));

    expect(await screen.findByText("Klipolo's server is busy right now. Try again in a moment.")).toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'Retry' }));
    expect(await screen.findByRole('button', { name: 'Analyze' })).toBeInTheDocument();
    expect(bridge.estimate).toHaveBeenCalledTimes(2);
    expect(bridge.start).toHaveBeenCalledTimes(1);
  });

  it('shows why nothing can pay yet with no price, and Retry estimates again without starting', async () => {
    const problem = "Klipolo's server is busy right now. Try again in a moment.";
    bridge.estimate.mockResolvedValueOnce(offer({ usd: 0, problem }));
    bridge.estimate.mockResolvedValueOnce(offer());
    showProject(CLIPS);
    renderWithProviders(<AnalysisBanner />);

    expect(await screen.findByText(problem)).toBeInTheDocument();
    expect(screen.queryByText(/Analyze 2 clips/)).toBeNull();
    expect(screen.queryByText(/\$/)).toBeNull();
    expect(screen.queryByRole('button', { name: 'Analyze' })).toBeNull();
    await userEvent.click(screen.getByRole('button', { name: 'Retry' }));
    expect(await screen.findByText('Analyze 2 clips for about $0.04')).toBeInTheDocument();
    expect(bridge.estimate).toHaveBeenCalledTimes(2);
    expect(bridge.start).not.toHaveBeenCalled();
  });

  it('lists each clip with its step while they run, and Cancel stops them', async () => {
    bridge.estimate.mockResolvedValue(offer());
    showProject(CLIPS);
    useAnalysis.setState({
      states: {
        a: { state: 'running', step: 'words', pct: 64 },
        b: { state: 'running', step: 'scenes', pct: 22 },
      },
    });
    renderWithProviders(<AnalysisBanner />);

    expect(screen.getByText('Analyzing 2 clips')).toBeInTheDocument();
    expect(screen.getByText('rosa-interview-a.mov')).toBeInTheDocument();
    expect(screen.getByText('Timing words')).toBeInTheDocument();
    expect(screen.getByText('64%')).toBeInTheDocument();
    expect(screen.getByText('Watching the footage')).toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'Cancel' }));
    expect(bridge.cancel).toHaveBeenCalled();
  });

  it('leaves a clip whose file is gone out of the offer and says what to do for it', async () => {
    bridge.estimate.mockResolvedValue(offer({ usd: 0.02, clips: ['a'], missing: ['b'] }));
    showProject(CLIPS);
    renderWithProviders(<AnalysisBanner />);

    expect(await screen.findByText('Analyze 1 clip for about $0.02')).toBeInTheDocument();
    expect(screen.getByText('rosa-wheel-broll.mov')).toBeInTheDocument();
    expect(screen.getByText(GONE)).toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'Analyze' }));
    expect(bridge.start).toHaveBeenCalledWith(['a']);
  });

  it('lists clips whose file is gone on their own when nothing else is offered, until Not now', async () => {
    bridge.estimate.mockResolvedValue(offer({ usd: 0, clips: [], missing: ['b'] }));
    showProject(CLIPS);
    const { container } = renderWithProviders(<AnalysisBanner />);

    expect(await screen.findByText("Klipolo can't find 1 clip")).toBeInTheDocument();
    expect(screen.getByText(GONE)).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Analyze' })).toBeNull();
    await userEvent.click(screen.getByRole('button', { name: 'Not now' }));
    expect(container).toBeEmptyDOMElement();
  });

  it('keeps a failed clip with its reason and Retry', async () => {
    bridge.estimate.mockResolvedValue(offer({ clips: [] }));
    showProject(CLIPS);
    const reason = 'This device has used its Klipolo credit. Add your own Meta API key in Settings to keep going.';
    useAnalysis.setState({ states: { a: { state: 'failed', reason }, b: { state: 'done', approximate: false } } });
    renderWithProviders(<AnalysisBanner />);

    expect(screen.getByText("Couldn't finish 1 clip")).toBeInTheDocument();
    expect(screen.getByRole('alert')).toHaveTextContent(reason);
    await userEvent.click(screen.getByRole('button', { name: 'Retry' }));
    expect(bridge.start).toHaveBeenCalledWith(['a']);
  });

  it('shows nothing once every clip is analysed', async () => {
    showProject(CLIPS);
    useAnalysis.setState({
      states: { a: { state: 'done', approximate: false }, b: { state: 'done', approximate: true } },
    });
    const { container } = renderWithProviders(<AnalysisBanner />);
    await act(async () => undefined);
    expect(container).toBeEmptyDOMElement();
    expect(bridge.estimate).not.toHaveBeenCalled();
  });

  it('notes rough captions in the preview header while words are timed', () => {
    useAnalysis.setState({ states: { a: { state: 'running', step: 'transcript', pct: 50 } } });
    const { rerender } = renderWithProviders(<RoughCaptionsNote />);
    expect(screen.queryByText('Rough captions until words are timed')).toBeNull();
    act(() => useAnalysis.setState({ states: { a: { state: 'running', step: 'words', pct: 10 } } }));
    rerender(<RoughCaptionsNote />);
    expect(screen.getByText('Rough captions until words are timed')).toBeInTheDocument();
  });

  it('offers the clips again after Cancel', async () => {
    bridge.estimate.mockResolvedValue(offer());
    showProject(CLIPS);
    useAnalysis.setState({
      states: { a: { state: 'running', step: 'words', pct: 10 }, b: { state: 'running', step: 'audio', pct: 0 } },
    });
    renderWithProviders(<AnalysisBanner />);
    expect(bridge.estimate).not.toHaveBeenCalled();

    act(() => useAnalysis.setState({ states: { a: { state: 'none' }, b: { state: 'none' } } }));
    expect(await screen.findByText('Analyze 2 clips for about $0.04')).toBeInTheDocument();
    expect(bridge.estimate).toHaveBeenCalledWith(['a', 'b']);
  });

  it('says when the words of a clip ended up timed roughly', () => {
    useAnalysis.setState({ states: { a: { state: 'done', approximate: false } } });
    const { rerender } = renderWithProviders(<RoughCaptionsNote />);
    expect(screen.queryByText(/timed roughly/)).toBeNull();
    act(() => useAnalysis.setState({ states: { a: { state: 'done', approximate: true } } }));
    rerender(<RoughCaptionsNote />);
    expect(screen.getByText('Some captions are timed roughly.')).toBeInTheDocument();
    act(() =>
      useAnalysis.setState({
        states: { a: { state: 'done', approximate: true }, b: { state: 'running', step: 'words', pct: 5 } },
      }),
    );
    rerender(<RoughCaptionsNote />);
    expect(screen.getByText('Rough captions until words are timed')).toBeInTheDocument();
  });

  it('says why captions are rough, and times the words again where that can help', async () => {
    const stopped = 'Word timing stopped before it finished.';
    useAnalysis.setState({
      states: {
        a: { state: 'done', approximate: true, approximateReason: stopped, timeAgain: true },
        // Words that don't match their sound: rough, and timing them again wouldn't help.
        b: { state: 'done', approximate: true, approximateReason: 'Some words are timed roughly.' },
        c: { state: 'done', approximate: false },
      },
    });
    renderWithProviders(<RoughCaptionsNote />);
    expect(screen.getByText('Some captions are timed roughly.')).toBeInTheDocument();
    expect(screen.getByText(stopped)).toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'Time words again' }));
    // As the person: the words are timed again whatever the page did before.
    expect(bridge.start).toHaveBeenCalledWith(['a']);
  });

  it('offers no way to time words again where it can’t help', () => {
    useAnalysis.setState({
      states: { b: { state: 'done', approximate: true, approximateReason: 'Some words are timed roughly.' } },
    });
    renderWithProviders(<RoughCaptionsNote />);
    expect(screen.getByText('Some captions are timed roughly.')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Time words again' })).toBeNull();
  });

  it('offers no way to time words again without a key or code, as it couldn’t run', () => {
    useAi.setState({ status: { mode: 'none' } });
    useAnalysis.setState({
      states: {
        a: {
          state: 'done',
          approximate: true,
          approximateReason: 'Word timing stopped before it finished.',
          timeAgain: true,
        },
      },
    });
    renderWithProviders(<RoughCaptionsNote />);
    expect(screen.getByText(/Word timing stopped before it finished\./)).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Time words again' })).toBeNull();
  });

  it('shows the reason in full, on a line of its own under the note', () => {
    const quota = "This device has used today's 120 minutes of word timing. It resets at midnight UTC.";
    useAnalysis.setState({
      states: { a: { state: 'done', approximate: true, approximateReason: quota, timeAgain: true } },
    });
    renderWithProviders(<RoughCaptionsNote />);
    const reason = screen.getByText(quota);
    expect(reason).toHaveClass('analysis-rough-reason');
    expect(reason).not.toHaveTextContent('Some captions are timed roughly.');
    // jsdom draws no CSS: the stylesheet itself must let the reason wrap, never cut it off with an ellipsis.
    const css = readFileSync(resolve(__dirname, 'analysis.css'), 'utf8');
    const rule = (selector: string) =>
      css.match(new RegExp(`(?:^|\\n)${selector.replace(/[.]/g, '\\.')}\\s*\\{([^}]*)\\}`))?.[1] ?? '';
    for (const selector of ['.analysis-rough.is-done', '.analysis-rough.is-done .analysis-rough-text']) {
      expect(rule(selector)).toMatch(/white-space:\s*normal/);
      expect(rule(selector)).toMatch(/overflow:\s*visible/);
    }
    expect(rule('.analysis-rough-reason')).not.toMatch(/nowrap|ellipsis/);
  });

  it('says why a Retry of a failed clip was refused', async () => {
    bridge.estimate.mockResolvedValue(offer({ clips: [] }));
    showProject(CLIPS);
    useAnalysis.setState({
      states: { a: { state: 'failed', reason: 'Meta is busy right now. Try again in a moment.' } },
    });
    const refusal =
      'Your access code has $0.00 left, and this needs about $0.04. Analyze fewer or shorter files, or add your own ' +
      'Meta key in Settings.';
    bridge.start.mockRejectedValueOnce(new Error(refusal));
    renderWithProviders(<AnalysisBanner />);
    await userEvent.click(screen.getByRole('button', { name: 'Retry' }));
    expect(await screen.findByText(refusal)).toBeInTheDocument();
  });
});

describe('a recording', () => {
  /** A take's files: its screen (the main footage) and camera, each a video with sound, and its mic. */
  const SCREEN = clip('screen', 'screen.mov');
  const CAMERA = clip('camera', 'camera.mov');
  const MIC = clip('mic', 'mic.m4a', 'audio');

  /**
   * A project whose main track plays one take of `sources` (each a role and its files, one run after another; the
   * first source is the main footage), whole, and each file of `alone` again as a clip of its own.
   */
  function showTake(sources: [SourceRole, ...Asset[]][], { alone = [] as string[] } = {}) {
    const recording = {
      id: 'take',
      kind: 'screen',
      sources: sources.map(([role, ...files]) => ({
        id: role,
        role,
        label: `capture.source.${role}`,
        runs: files.map((file, k) => ({
          assetId: file.id,
          offsetUs: k * file.durationUs,
          durationUs: file.durationUs,
          driftPpm: 0,
        })),
        nudgeUs: 0,
        mix: { volume: 1, muted: false, effects: [] },
      })),
      primary: sources[0][0],
      markers: [],
    };
    const assets = sources.flatMap(([, ...files]) => files);
    const end = Math.max(...sources.map(([, ...files]) => files.reduce((sum, file) => sum + file.durationUs, 0)));
    const whole = { sourceInUs: 0, sourceOutUs: end };
    const main = [
      { id: 'clip-take', source: { type: 'recording', id: 'take' }, ...whole },
      ...alone.map((id) => ({ id: `clip-${id}`, source: { type: 'asset', id }, ...whole })),
    ];
    const snapshot = {
      project: { assets, recordings: [recording], timeline: { main, tracks: [] } },
    } as unknown as Snapshot;
    useProject.setState({ snapshot });
  }

  /** Every asset id the page sent to Rust to estimate or analyse. */
  function sent(): string[] {
    return [...bridge.estimate.mock.calls, ...bridge.start.mock.calls].flatMap(([ids]) => ids);
  }

  it('a_recording_counts_as_one_clip_in_the_offer', async () => {
    bridge.estimate.mockResolvedValue(offer({ usd: 0.01, clips: ['mic'] }));
    showTake([
      ['screen', SCREEN],
      ['camera', CAMERA],
      ['mic', MIC],
    ]);
    renderWithProviders(<AnalysisBanner />);

    expect(await screen.findByText('Analyze 1 clip for about $0.01')).toBeInTheDocument();
    expect(bridge.estimate).toHaveBeenCalledTimes(1);
    expect(bridge.estimate).toHaveBeenCalledWith(['mic']);
    // Its words only: a take's footage isn't watched.
    expect(screen.getByText(/^Klipolo transcribes it and times each word\./)).toBeInTheDocument();

    // While it runs, its row is the take, named as its clip is.
    act(() => useAnalysis.setState({ states: { mic: { state: 'running', step: 'words', pct: 40 } } }));
    expect(screen.getByText('Analyzing 1 clip')).toBeInTheDocument();
    expect(screen.getByText('Recording 1')).toBeInTheDocument();
    expect(screen.getByText('Timing words')).toBeInTheDocument();
  });

  it('screen_and_camera_files_are_never_sent', async () => {
    bridge.estimate.mockResolvedValue(offer({ usd: 0.01, clips: ['mic'] }));
    showTake([
      ['screen', SCREEN],
      ['camera', CAMERA],
      ['mic', MIC],
    ]);
    const { container } = renderWithProviders(<AnalysisBanner />);

    expect(await screen.findByText('Analyze 1 clip for about $0.01')).toBeInTheDocument();
    // Analyze from the Captions panel or chat offers the same.
    await act(() => offerAgain());
    expect(bridge.estimate).toHaveBeenLastCalledWith(['mic']);
    await userEvent.click(screen.getByRole('button', { name: 'Analyze' }));
    expect(bridge.start).toHaveBeenCalledWith(['mic']);
    // Once its voice is analysed, the take has nothing left to offer.
    act(() => useAnalysis.setState({ states: { mic: { state: 'done', approximate: false } } }));
    await act(async () => undefined);
    expect(container).toBeEmptyDOMElement();

    expect(sent().length).toBeGreaterThan(0);
    expect(sent()).not.toContain('screen');
    expect(sent()).not.toContain('camera');
  });

  it('a_take_with_no_mic_offers_the_camera_s_words_only', async () => {
    bridge.estimate.mockResolvedValue(offer({ usd: 0.01, clips: ['camera'] }));
    showTake([
      ['screen', { ...SCREEN, hasAudio: false }],
      ['camera', CAMERA],
    ]);
    renderWithProviders(<AnalysisBanner />);

    expect(await screen.findByText('Analyze 1 clip for about $0.01')).toBeInTheDocument();
    expect(bridge.estimate).toHaveBeenCalledWith(['camera']);
    expect(screen.getByText(/^Klipolo transcribes it and times each word\./)).toBeInTheDocument();
  });

  it('a_file_of_a_take_placed_on_its_own_is_offered_for_its_words', async () => {
    bridge.estimate.mockResolvedValue(offer({ usd: 0.02, clips: ['screen', 'mic'] }));
    showTake(
      [
        ['screen', SCREEN],
        ['camera', CAMERA],
        ['mic', MIC],
      ],
      { alone: ['screen'] },
    );
    renderWithProviders(<AnalysisBanner />);

    expect(await screen.findByText('Analyze 2 clips for about $0.02')).toBeInTheDocument();
    expect(bridge.estimate).toHaveBeenCalledWith(['screen', 'mic']);
    expect(screen.getByText(/^Klipolo transcribes them and times each word\./)).toBeInTheDocument();

    // Its row is named as the timeline names that clip.
    act(() => useAnalysis.setState({ states: { screen: { state: 'running', step: 'audio', pct: 0 } } }));
    expect(screen.getByText('Screen of Recording 1')).toBeInTheDocument();
  });

  it('a_camera_take_is_one_clip_and_one_row_for_its_words_and_its_scene_log', async () => {
    // "Record yourself": its camera, the footage, is watched; its mic, the voice, heard. One take, one clip.
    const SILENT = { ...CAMERA, hasAudio: false };
    bridge.estimate.mockResolvedValue(offer({ usd: 0.05, clips: ['camera', 'mic'] }));
    showTake([
      ['camera', SILENT],
      ['mic', MIC],
    ]);
    renderWithProviders(<AnalysisBanner />);

    expect(await screen.findByText('Analyze 1 clip for about $0.05')).toBeInTheDocument();
    expect(bridge.estimate).toHaveBeenCalledWith(['camera', 'mic']);
    expect(screen.getByText(/^Klipolo transcribes it, times each word and watches the footage\./)).toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'Analyze' }));
    expect(bridge.start).toHaveBeenCalledWith(['camera', 'mic']);

    // One row named as its clip, at the step of its least advanced file.
    act(() =>
      useAnalysis.setState({
        states: {
          camera: { state: 'running', step: 'scenes', pct: 10 },
          mic: { state: 'running', step: 'words', pct: 50 },
        },
      }),
    );
    expect(screen.getByText('Analyzing 1 clip')).toBeInTheDocument();
    expect(screen.getAllByText('Recording 1')).toHaveLength(1);
    expect(screen.queryByText('Camera of Recording 1')).toBeNull();
    expect(screen.getByText('Timing words')).toBeInTheDocument();
  });

  it('a_take_in_two_files_counts_and_lists_as_one_clip', async () => {
    // The mic reconnected into a second file: still one take, one clip.
    const MIC2 = clip('mic2', 'mic-2.m4a', 'audio');
    bridge.estimate.mockResolvedValue(offer({ usd: 0.02, clips: ['mic', 'mic2'] }));
    showTake([
      ['screen', { ...SCREEN, hasAudio: false }],
      ['mic', MIC, MIC2],
    ]);
    renderWithProviders(<AnalysisBanner />);

    expect(await screen.findByText('Analyze 1 clip for about $0.02')).toBeInTheDocument();
    expect(bridge.estimate).toHaveBeenCalledWith(['mic', 'mic2']);
    expect(screen.getByText(/^Klipolo transcribes it and times each word\./)).toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'Analyze' }));
    expect(bridge.start).toHaveBeenCalledWith(['mic', 'mic2']);

    // One row, at the step of its least advanced file, and the screen reader hears the name the row shows.
    act(() =>
      useAnalysis.setState({
        states: {
          mic: { state: 'running', step: 'words', pct: 80 },
          mic2: { state: 'running', step: 'transcript', pct: 30 },
        },
      }),
    );
    expect(screen.getByText('Analyzing 1 clip')).toBeInTheDocument();
    expect(screen.getAllByText('Recording 1')).toHaveLength(1);
    expect(screen.getByText('Transcribing')).toBeInTheDocument();
    expect(screen.getByText('30%')).toBeInTheDocument();
    expect(screen.getByRole('status')).toHaveTextContent('Recording 1: Transcribing, 25%');

    // One file failed and the other is done: one take couldn't finish, and Retry asks for its failed file.
    const busy = 'Meta is busy right now. Try again in a moment.';
    act(() =>
      useAnalysis.setState({
        states: { mic: { state: 'done', approximate: false }, mic2: { state: 'failed', reason: busy } },
      }),
    );
    expect(screen.getByText("Couldn't finish 1 clip")).toBeInTheDocument();
    expect(screen.getAllByText('Recording 1')).toHaveLength(1);
    await userEvent.click(screen.getByRole('button', { name: 'Retry' }));
    expect(bridge.start).toHaveBeenLastCalledWith(['mic2']);
  });
});
