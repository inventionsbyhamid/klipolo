import { create } from 'zustand';
import type { AssetState } from '../bindings/AssetState';
import type { Estimate } from '../bindings/Estimate';
import { refreshAiStatus } from '../ai/aiStatus';
import { analysisBridge, type AnalysisEvent, type UnlistenFn } from '../bridge/analysis';
import { useProject } from './project';

interface AnalysisStore {
  /** Each clip's analysis as Rust last told it, by asset id; a clip missing here is not analysed. */
  states: Record<string, AssetState>;
  /** What analysing the clips not analysed yet costs; null until asked, or when none needs it. */
  estimate: Estimate | null;
  /** Why Analyze couldn't start (the device isn't registered, the credit ran short), shown with Retry. */
  error: string | null;
  /** The clips the offer was put off for ("Not now"), until other clips come. */
  dismissed: string[];
  /** An estimate or a start is on its way. */
  busy: boolean;
}

/**
 * Every clip's analysis, for the analysis banner and, later, chat (which reads whether a clip is analysed). Rust tells
 * every change (`analysis://progress`, `asset_done`, `failed`, `stopped`), so no surface asks on its own.
 */
export const useAnalysis = create<AnalysisStore>(() => ({
  states: {},
  estimate: null,
  error: null,
  dismissed: [],
  busy: false,
}));

/** A clip's analysis: not analysed when Rust hasn't said. */
export function stateOf(states: Record<string, AssetState>, assetId: string): AssetState {
  return states[assetId] ?? { state: 'none' };
}

/**
 * One event from Rust, into the store. A clip done or failed spent credit: the AI status (the top bar's credit) is read
 * again.
 */
export function applyEvent(event: AnalysisEvent) {
  const { assetId } = event.payload;
  let next: AssetState;
  switch (event.kind) {
    case 'progress':
      next = { state: 'running', step: event.payload.step, pct: event.payload.pct };
      break;
    case 'done':
      next = { state: 'done', approximate: event.payload.approximate };
      break;
    case 'failed':
      next = { state: 'failed', reason: event.payload.reason };
      break;
    case 'stopped':
      next = { state: 'none' };
      break;
  }
  useAnalysis.setState((s) => ({ states: { ...s.states, [assetId]: next } }));
  if (event.kind === 'done' || event.kind === 'failed') void refreshAiStatus();
}

/** Reads every clip's state once and follows every change after it. */
export async function followAnalysis(): Promise<UnlistenFn> {
  const unlisten = await analysisBridge.onEvent(applyEvent);
  try {
    const known = await analysisBridge.states();
    useAnalysis.setState((s) => ({
      // An event that came while the list was read is newer than it.
      states: { ...Object.fromEntries(known.map((k) => [k.assetId, k.state])), ...s.states },
    }));
  } catch {
    // Nothing known yet: every clip counts as not analysed.
  }
  return unlisten;
}

/**
 * Another project opened (or this one opened again): the store forgets the last one's clips, offer and sentences, as
 * Rust does (`project_changed`), so no progress or rough-captions note carries over. Gives the stop function.
 */
export function clearAnalysisOnProjectSwitch(): () => void {
  return useProject.subscribe((s, prev) => {
    if (s.snapshot?.project.id !== prev.snapshot?.project.id || s.snapshot?.opening !== prev.snapshot?.opening) {
      asked += 1;
      useAnalysis.setState({ states: {}, estimate: null, error: null, dismissed: [], busy: false });
    }
  });
}

/** Offers asked for, so an older estimate never replaces a newer one. */
let asked = 0;

/**
 * The clips `assetIds` (the project's videos and sounds) as the banner offers them: the ones not analysed are
 * estimated; those cached load at once, at no cost (`analysis_start`), and the offer is for the rest. A new offer
 * starts without the last one's sentence; when nothing can pay yet (the device isn't registered), the estimate's
 * problem is the sentence, shown with Retry.
 */
export async function offerFor(assetIds: string[]) {
  const { states } = useAnalysis.getState();
  const fresh = assetIds.filter((id) => stateOf(states, id).state === 'none');
  const ask = ++asked;
  if (fresh.length === 0) {
    useAnalysis.setState({ estimate: null, error: null });
    return;
  }
  useAnalysis.setState({ busy: true, error: null });
  try {
    const estimate = await analysisBridge.estimate(fresh);
    if (ask !== asked) return;
    useAnalysis.setState({ estimate, busy: false, error: estimate.problem ?? null });
    if (estimate.cached.length > 0) await analysisBridge.start(estimate.cached).catch(() => undefined);
  } catch {
    if (ask === asked) useAnalysis.setState({ estimate: null, busy: false });
  }
}

/** Analyze: the clips the estimate offers. A refusal comes back as the sentence to show, with Retry. */
export async function analyze() {
  const clips = useAnalysis.getState().estimate?.clips ?? [];
  if (clips.length === 0) return;
  useAnalysis.setState({ busy: true, error: null });
  try {
    await analysisBridge.start(clips);
    useAnalysis.setState({ busy: false, estimate: null });
  } catch (error) {
    useAnalysis.setState({ busy: false, error: error instanceof Error ? error.message : String(error) });
  }
}

/** Retry the clips whose analysis failed (what finished is kept, so only the rest is done again). */
export async function retry(assetIds: string[]) {
  useAnalysis.setState({ error: null });
  try {
    await analysisBridge.start(assetIds);
  } catch (error) {
    useAnalysis.setState({ error: error instanceof Error ? error.message : String(error) });
  }
}

/** Cancel: Rust stops after the requests in flight, and tells each clip it stopped. */
export function cancelAnalysis() {
  void analysisBridge.cancel().catch(() => undefined);
}

/** "Not now": the offer goes until other clips come. */
export function notNow() {
  const clips = useAnalysis.getState().estimate?.clips ?? [];
  useAnalysis.setState((s) => ({ dismissed: [...new Set([...s.dismissed, ...clips])], error: null }));
}
