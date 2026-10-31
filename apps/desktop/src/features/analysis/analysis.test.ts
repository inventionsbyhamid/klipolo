import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Estimate } from '../../bindings/Estimate';

vi.mock('../../ai/aiStatus', () => ({ refreshAiStatus: vi.fn() }));
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
const { refreshAiStatus } = await import('../../ai/aiStatus');
const { analyze, applyEvent, clearAnalysisOnProjectSwitch, followAnalysis, notNow, offerFor, retry, stateOf, useAnalysis } =
  await import('../../state/analysis');
const { useProject } = await import('../../state/project');
const { about, howLong, offerText, offerTitle, progressTitle, spokenProgress, stepDots } = await import('./analysis');

const bridge = vi.mocked(analysisBridge);

function estimate(over: Partial<Estimate> = {}): Estimate {
  return { usd: 0.04, seconds: 130, cached: [], clips: ['a', 'b'], ...over };
}

beforeEach(() => {
  vi.clearAllMocks();
  useAnalysis.setState({ states: {}, estimate: null, error: null, dismissed: [], busy: false });
});

describe('the offer', () => {
  it('says what it costs, plainly', () => {
    expect(offerTitle(estimate())).toBe('Analyze 2 clips for about $0.04');
    expect(offerTitle(estimate({ clips: ['a'], usd: 0.004 }))).toBe('Analyze 1 clip for less than $0.01');
    expect(about(0.0749)).toBe('about $0.07');
  });

  it('says what Analyze does, how long it takes, and the credit left', () => {
    expect(offerText(estimate({ creditLeftUsd: 1.94 }), true)).toBe(
      'Klipolo transcribes them, times each word and watches the footage. Takes about 2 minutes. You have $1.94 ' +
        'left on your access code.',
    );
    expect(offerText(estimate({ clips: ['a'], seconds: 20 }), false)).toBe(
      'Klipolo transcribes it and times each word. Takes about a minute.',
    );
    expect(offerText(estimate({ cached: ['c'] }), true)).toContain('1 clip analyzed before loads at no cost.');
  });

  it('gives the refusal instead of the credit', () => {
    const refusal =
      'Your access code has $0.02 left, and this needs about $0.07. Trim the clips first, or add your own Meta key in ' +
      'Settings.';
    const text = offerText(estimate({ creditLeftUsd: 0.02, refusal }), true);
    expect(text.endsWith(refusal)).toBe(true);
    expect(text).not.toContain('You have');
  });

  it('never uses a long dash', () => {
    const all = [
      offerTitle(estimate()),
      offerText(estimate({ creditLeftUsd: 1, cached: ['c', 'd'] }), true),
      howLong(3600),
      progressTitle(2, 0),
      progressTitle(0, 1),
    ].join(' ');
    expect(all).not.toMatch(/[–—]/);
  });
});

describe('progress', () => {
  it('fills the dots up to the step running', () => {
    expect(stepDots('words', 64)).toEqual([
      { kind: 'done' },
      { kind: 'done' },
      { kind: 'now', pct: 64 },
      { kind: 'next' },
    ]);
    expect(stepDots('audio', 120)[0]).toEqual({ kind: 'now', pct: 100 });
  });

  it('is announced every 25%, not every percent', () => {
    expect(spokenProgress('a.mov', 'scenes', 49)).toBe('a.mov: Watching the footage, 25%');
    expect(spokenProgress('a.mov', 'transcript', 50)).toBe('a.mov: Transcribing, 50%');
  });

  it('counts the clips running, then the ones that failed', () => {
    expect(progressTitle(2, 1)).toBe('Analyzing 2 clips');
    expect(progressTitle(0, 1)).toBe("Couldn't finish 1 clip");
  });
});

describe('the store', () => {
  it('follows Rust’s events', () => {
    applyEvent({ kind: 'progress', payload: { assetId: 'a', step: 'transcript', pct: 40 } });
    expect(stateOf(useAnalysis.getState().states, 'a')).toEqual({ state: 'running', step: 'transcript', pct: 40 });
    applyEvent({ kind: 'done', payload: { assetId: 'a', approximate: true } });
    expect(stateOf(useAnalysis.getState().states, 'a')).toEqual({ state: 'done', approximate: true });
    applyEvent({ kind: 'failed', payload: { assetId: 'b', reason: 'This device has used its Klipolo credit.' } });
    expect(stateOf(useAnalysis.getState().states, 'b').state).toBe('failed');
    applyEvent({ kind: 'stopped', payload: { assetId: 'b' } });
    expect(stateOf(useAnalysis.getState().states, 'b')).toEqual({ state: 'none' });
    expect(stateOf(useAnalysis.getState().states, 'never')).toEqual({ state: 'none' });
  });

  it('reads the credit again when a clip is done or failed (it was spent)', () => {
    applyEvent({ kind: 'progress', payload: { assetId: 'a', step: 'scenes', pct: 90 } });
    expect(refreshAiStatus).not.toHaveBeenCalled();
    applyEvent({ kind: 'done', payload: { assetId: 'a', approximate: false } });
    applyEvent({ kind: 'failed', payload: { assetId: 'b', reason: 'Meta is busy right now. Try again in a moment.' } });
    expect(refreshAiStatus).toHaveBeenCalledTimes(2);
  });

  it('reads what Rust knows once, and an event that came meanwhile wins', async () => {
    bridge.onEvent.mockImplementation(async (handler) => {
      handler({ kind: 'progress', payload: { assetId: 'a', step: 'words', pct: 10 } });
      return () => undefined;
    });
    bridge.states.mockResolvedValue([
      { assetId: 'a', state: { state: 'none' } },
      { assetId: 'b', state: { state: 'done', approximate: false } },
    ]);
    await followAnalysis();
    const { states } = useAnalysis.getState();
    expect(states.a).toEqual({ state: 'running', step: 'words', pct: 10 });
    expect(states.b).toEqual({ state: 'done', approximate: false });
  });

  it('loads cached clips at once and offers the rest', async () => {
    bridge.estimate.mockResolvedValue(estimate({ cached: ['c'], clips: ['a'] }));
    bridge.start.mockResolvedValue(undefined);
    useAnalysis.setState({ states: { d: { state: 'done', approximate: false } } });
    await offerFor(['a', 'c', 'd']);
    expect(bridge.estimate).toHaveBeenCalledWith(['a', 'c']);
    // The page's own start: rough words are timed again at most once per app run.
    expect(bridge.start).toHaveBeenCalledWith(['c'], true);
    expect(useAnalysis.getState().estimate?.clips).toEqual(['a']);
  });

  it('keeps why nothing can pay yet in the estimate, and starts each offer without the last sentence', async () => {
    const problem = "Klipolo's server is busy right now. Try again in a moment.";
    useAnalysis.setState({ error: 'Meta is busy right now. Try again in a moment.' });
    bridge.estimate.mockResolvedValueOnce(estimate({ usd: 0, problem }));
    await offerFor(['a', 'b']);
    expect(useAnalysis.getState().estimate?.problem).toBe(problem);
    expect(useAnalysis.getState().error).toBeNull();
  });

  it('forgets the last project’s clips when another opens', () => {
    const stop = clearAnalysisOnProjectSwitch();
    const snapshot = (id: string, opening: number) => ({ project: { id }, opening }) as never;
    useProject.setState({ snapshot: snapshot('one', 1) });
    useAnalysis.setState({
      states: { a: { state: 'running', step: 'words', pct: 30 } },
      estimate: estimate(),
      error: 'Meta is busy right now. Try again in a moment.',
      dismissed: ['b'],
    });
    useProject.setState({ snapshot: snapshot('one', 1) });
    expect(useAnalysis.getState().states.a).toBeDefined();
    useProject.setState({ snapshot: snapshot('two', 2) });
    expect(useAnalysis.getState()).toMatchObject({ states: {}, estimate: null, error: null, dismissed: [] });
    stop();
  });

  it('asks nothing when every clip is analysed', async () => {
    useAnalysis.setState({ states: { a: { state: 'done', approximate: false } }, estimate: estimate() });
    await offerFor(['a']);
    expect(bridge.estimate).not.toHaveBeenCalled();
    expect(useAnalysis.getState().estimate).toBeNull();
  });

  it('keeps the sentence when Analyze is refused, for Retry', async () => {
    useAnalysis.setState({ estimate: estimate() });
    bridge.start.mockRejectedValueOnce(new Error("Couldn't reach Klipolo to set up this Mac."));
    await analyze();
    expect(bridge.start).toHaveBeenCalledWith(['a', 'b']);
    expect(useAnalysis.getState().error).toBe("Couldn't reach Klipolo to set up this Mac.");
    bridge.start.mockResolvedValueOnce(undefined);
    await analyze();
    expect(useAnalysis.getState().error).toBeNull();
    expect(useAnalysis.getState().estimate).toBeNull();
  });

  it('retries a failed clip, and puts the offer off with Not now', async () => {
    bridge.start.mockResolvedValue(undefined);
    await retry(['b']);
    expect(bridge.start).toHaveBeenCalledWith(['b']);
    useAnalysis.setState({ estimate: estimate() });
    notNow();
    expect(useAnalysis.getState().dismissed).toEqual(['a', 'b']);
  });
});
