import { act, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Asset } from '../../bindings/Asset';
import type { Estimate } from '../../bindings/Estimate';
import type { Snapshot } from '../../bindings/Snapshot';
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
const { useAnalysis } = await import('../../state/analysis');
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

function showProject(assets: Asset[]) {
  const snapshot = { project: { assets } } as unknown as Snapshot;
  useProject.setState({ snapshot });
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
    expect(screen.getByText(`Some captions are timed roughly. ${stopped}`)).toBeInTheDocument();
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
