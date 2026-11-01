import { create } from 'zustand';
import type { Asset } from '../bindings/Asset';
import type { AssetState } from '../bindings/AssetState';
import type { Estimate } from '../bindings/Estimate';
import { refreshAiStatus, useAi } from '../ai/aiStatus';
import { analysisBridge, type AnalysisEvent, type UnlistenFn } from '../bridge/analysis';
import { analysable } from '../lib/analysed';
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
    case 'done': {
      // Why some words are rough (and whether timing them again can help), or why there are none.
      const { approximate, approximateReason, emptyReason, timeAgain } = event.payload;
      next = { state: 'done', approximate, approximateReason, emptyReason, timeAgain };
      break;
    }
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

/**
 * A clip removed from the open project (not the project switching): the store forgets it, so it no longer counts for
 * the banner or the rough-captions note, and Rust stops telling it anything (`analysis_forget`). Its file's analysis
 * carries on for any other clip on it. Gives the stop function.
 */
export function forgetRemovedClips(): () => void {
  return useProject.subscribe((s, prev) => {
    const now = s.snapshot;
    const before = prev.snapshot;
    if (!now || !before || now.project.id !== before.project.id || now.opening !== before.opening) return;
    if (now.project.assets === before.project.assets) return;
    const kept = new Set(now.project.assets.map((a) => a.id));
    const gone = before.project.assets.map((a) => a.id).filter((id) => !kept.has(id));
    if (gone.length === 0) return;
    useAnalysis.setState((st) => ({
      states: Object.fromEntries(Object.entries(st.states).filter(([id]) => kept.has(id))),
    }));
    void analysisBridge.forget(gone).catch(() => undefined);
  });
}

/** Offers asked for, so an older estimate never replaces a newer one. */
let asked = 0;

/**
 * The clips `assetIds` (what analysis offers, `analysable`) as the banner offers them: the ones not analysed are
 * estimated; those cached load at once, at no cost (`analysis_start`), and the offer is for the rest. A new offer
 * starts without the last one's sentence. When nothing can pay yet (the device isn't registered), the estimate says
 * why (`Estimate.problem`) and has no price: the banner shows that, and Retry asks again.
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
    useAnalysis.setState({ estimate, busy: false });
    if (estimate.cached.length > 0) await analysisBridge.start(estimate.cached, true).catch(() => undefined);
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

/**
 * Retry the clips whose analysis failed (what finished is kept, so only the rest is done again), or time a done clip's
 * rough words again ("Time words again": free, and Rust always times them for the person).
 */
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

/** "Not now": the offer (and the clips whose file is gone) goes until other clips come. */
export function notNow() {
  const { clips = [], missing = [] } = useAnalysis.getState().estimate ?? {};
  useAnalysis.setState((s) => ({ dismissed: [...new Set([...s.dismissed, ...clips, ...missing])], error: null }));
}

const NO_ASSETS: Asset[] = [];

/**
 * The open project's clips analysis is for (`analysable`: the videos and sounds its main-track clips play, a recording
 * counted as one clip by its voice; never music, sounds or pictures on layers, nor files never placed). The same array
 * until they change.
 */
export function useAnalysedClips(): Asset[] {
  return useProject((s) => (s.snapshot ? analysable(s.snapshot.project) : NO_ASSETS));
}

/**
 * Whether a clip analysis is for (`useAnalysedClips`) is not analysed (never, or stopped): what Analyze (`offerAgain`)
 * offers. A clip that failed isn't: the banner's Retry is its way on.
 */
export function useSomeNotAnalysed(): boolean {
  const clips = useAnalysedClips();
  return useAnalysis((s) => clips.some((a) => stateOf(s.states, a.id).state === 'none'));
}

/** The open project's clips analysis is for, by id (`analysable`). */
function projectClipIds(): string[] {
  const project = useProject.getState().snapshot?.project;
  return project ? analysable(project).map((a) => a.id) : [];
}

/**
 * Analyze from outside the banner (the Captions panel with no words, chat's note): "Not now" is forgotten and the
 * banner offers the clips not analysed again, with their price.
 */
export function offerAgain(): Promise<void> {
  useAnalysis.setState({ dismissed: [] });
  return offerFor(projectClipIds());
}

/**
 * The way to pay changed (a key or code added or removed): "Not now" is forgotten, so the banner's new offer (asked for
 * by the banner on the same change) shows. Gives the stop function.
 */
export function offerAgainOnAiModeChange(): () => void {
  return useAi.subscribe((s, prev) => {
    if (s.status?.mode !== prev.status?.mode) useAnalysis.setState({ dismissed: [] });
  });
}

/**
 * Why the open project has no caption words, for the Captions panel: clips not analysed yet (the panel offers
 * Analyze), or clips heard with no speech in them (one, or more); null while clips are being analysed, or with no
 * clips.
 */
export function whyNoWords(
  states: Record<string, AssetState>,
  clipIds: string[],
): 'notAnalysed' | 'noSpeechOne' | 'noSpeechMany' | null {
  const all = clipIds.map((id) => stateOf(states, id));
  if (all.some((s) => s.state === 'running')) return null;
  if (all.some((s) => s.state === 'none')) return 'notAnalysed';
  const silent = all.filter((s) => s.state === 'done' && !!s.emptyReason).length;
  if (silent === 0) return null;
  return silent === 1 ? 'noSpeechOne' : 'noSpeechMany';
}
