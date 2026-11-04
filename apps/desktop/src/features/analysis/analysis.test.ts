import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Estimate } from '../../bindings/Estimate';

vi.mock('../../ai/aiStatus', async (real) => ({ ...(await real<object>()), creditMoved: vi.fn() }));
vi.mock('../../bridge/analysis', () => ({
  analysisBridge: {
    estimate: vi.fn(),
    start: vi.fn(),
    cancel: vi.fn(),
    states: vi.fn(),
    forget: vi.fn(),
    onEvent: vi.fn(),
  },
}));
const { analysisBridge } = await import('../../bridge/analysis');
const { creditMoved, useAi } = await import('../../ai/aiStatus');
const {
  analyze,
  applyEvent,
  clearAnalysisOnProjectSwitch,
  followAnalysis,
  forgetRemovedClips,
  notNow,
  offerAgain,
  offerAgainOnAiModeChange,
  offerFor,
  retry,
  stateOf,
  useAnalysis,
  whyNoWords,
} = await import('../../state/analysis');
const { useProject } = await import('../../state/project');
const { about, howLong, offerText, offerTitle, progressTitle, spokenProgress, stepDots } = await import('./analysis');

const bridge = vi.mocked(analysisBridge);

function estimate(over: Partial<Estimate> = {}): Estimate {
  return { usd: 0.04, seconds: 130, cached: [], clips: ['a', 'b'], missing: [], ...over };
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

  it('says the free credit left, with no key needed', () => {
    expect(offerText(estimate({ creditLeftUsd: 4.8 }), false, true)).toBe(
      'Klipolo transcribes them and times each word. Takes about 2 minutes. Free credit: $4.80 left. No key needed.',
    );
  });

  it('gives the refusal instead of the credit', () => {
    const refusal =
      'Your access code has $0.02 left, and this needs about $0.07. Analyze fewer or shorter files, or add your own ' +
      'Meta key in Settings.';
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
    expect(all).not.toMatch(/[\u2013\u2014]/);
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
    // Why words are rough (and whether timing them again helps), or why there are none, come with it.
    const stopped = 'Word timing stopped before it finished.';
    applyEvent({
      kind: 'done',
      payload: { assetId: 'c', approximate: true, approximateReason: stopped, timeAgain: true },
    });
    expect(stateOf(useAnalysis.getState().states, 'c')).toEqual({
      state: 'done',
      approximate: true,
      approximateReason: stopped,
      timeAgain: true,
    });
    applyEvent({ kind: 'done', payload: { assetId: 'q', approximate: false, emptyReason: 'No speech found.' } });
    expect(stateOf(useAnalysis.getState().states, 'q')).toEqual({
      state: 'done',
      approximate: false,
      emptyReason: 'No speech found.',
    });
    applyEvent({ kind: 'failed', payload: { assetId: 'b', reason: 'This device has used its Klipolo credit.' } });
    expect(stateOf(useAnalysis.getState().states, 'b').state).toBe('failed');
    applyEvent({ kind: 'stopped', payload: { assetId: 'b' } });
    expect(stateOf(useAnalysis.getState().states, 'b')).toEqual({ state: 'none' });
    expect(stateOf(useAnalysis.getState().states, 'never')).toEqual({ state: 'none' });
  });

  it('reads the credit again when a clip is done or failed (it was spent)', () => {
    applyEvent({ kind: 'progress', payload: { assetId: 'a', step: 'scenes', pct: 90 } });
    expect(creditMoved).not.toHaveBeenCalled();
    applyEvent({ kind: 'done', payload: { assetId: 'a', approximate: false } });
    applyEvent({ kind: 'failed', payload: { assetId: 'b', reason: 'Meta is busy right now. Try again in a moment.' } });
    expect(creditMoved).toHaveBeenCalledTimes(2);
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

  it('forgets a clip removed from the project, and tells Rust, keeping the others', () => {
    bridge.forget.mockResolvedValue(undefined);
    const stop = forgetRemovedClips();
    const snapshot = (ids: string[]) =>
      ({ project: { id: 'one', assets: ids.map((id) => ({ id })) }, opening: 1 }) as never;
    useProject.setState({ snapshot: snapshot(['a', 'b']) });
    useAnalysis.setState({
      states: { a: { state: 'done', approximate: true }, b: { state: 'done', approximate: true } },
    });
    useProject.setState({ snapshot: snapshot(['a']) });
    expect(Object.keys(useAnalysis.getState().states)).toEqual(['a']);
    expect(bridge.forget).toHaveBeenCalledWith(['b']);
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

  it('offers the clips again after Not now once the way to pay changes', () => {
    const stop = offerAgainOnAiModeChange();
    useAi.setState({ status: { mode: 'none' } });
    useAnalysis.setState({ dismissed: ['a', 'b'] });
    useAi.setState({ status: { mode: 'none' } });
    expect(useAnalysis.getState().dismissed).toEqual(['a', 'b']);
    // An access code added in the top bar.
    useAi.setState({ status: { mode: 'relay', budgetUsd: 2, spentUsd: 0, active: true } });
    expect(useAnalysis.getState().dismissed).toEqual([]);
    stop();
  });

  it('Analyze from outside the banner forgets Not now and offers the project’s clips again', async () => {
    bridge.estimate.mockResolvedValue(estimate());
    const assets = [
      { id: 'a', kind: 'video' },
      { id: 'b', kind: 'audio' },
      { id: 'logo', kind: 'image' },
      // Music on a layer, and a song never placed: no clip on the main track plays them.
      { id: 'bed', kind: 'audio' },
      { id: 'song', kind: 'audio' },
    ];
    const main = ['a', 'b', 'logo'].map((id) => ({ id: `clip-${id}`, source: { type: 'asset', id } }));
    const timeline = { main, tracks: [] };
    const project = { id: 'one', assets, recordings: [], timeline };
    useProject.setState({ snapshot: { project, opening: 1 } as never });
    useAnalysis.setState({ dismissed: ['a', 'b'] });
    await offerAgain();
    expect(useAnalysis.getState().dismissed).toEqual([]);
    expect(bridge.estimate).toHaveBeenCalledWith(['a', 'b']);
    expect(useAnalysis.getState().estimate?.clips).toEqual(['a', 'b']);
  });

  it('says why a project has no caption words', () => {
    const silent = { state: 'done', approximate: false, emptyReason: 'No speech found.' } as const;
    const timed = { state: 'done', approximate: false } as const;
    expect(whyNoWords({ a: silent }, ['a', 'b'])).toBe('notAnalysed');
    expect(whyNoWords({ a: silent, b: { state: 'running', step: 'words', pct: 4 } }, ['a', 'b'])).toBeNull();
    expect(whyNoWords({ a: silent, b: timed }, ['a', 'b'])).toBe('noSpeechOne');
    expect(whyNoWords({ a: silent, b: silent }, ['a', 'b'])).toBe('noSpeechMany');
    expect(whyNoWords({ a: timed }, ['a'])).toBeNull();
    expect(whyNoWords({}, [])).toBeNull();
    // A take's two files with no speech are one clip with none.
    const takes = new Map([
      ['mic', 'take'],
      ['mic2', 'take'],
    ]);
    expect(whyNoWords({ mic: silent, mic2: silent }, ['mic', 'mic2'], takes)).toBe('noSpeechOne');
    expect(whyNoWords({ mic: silent, mic2: silent, b: silent }, ['mic', 'mic2', 'b'], takes)).toBe('noSpeechMany');
    // A camera take: its silent camera (the footage) and its mic with no speech are one clip with none.
    const camera = new Map([
      ['mic', 'take'],
      ['camera', 'take'],
    ]);
    expect(whyNoWords({ camera: silent, mic: silent }, ['camera', 'mic'], camera)).toBe('noSpeechOne');
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
