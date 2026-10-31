import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type { AnalysisDone } from '../bindings/AnalysisDone';
import type { AnalysisFailed } from '../bindings/AnalysisFailed';
import type { AnalysisProgress } from '../bindings/AnalysisProgress';
import type { AnalysisStopped } from '../bindings/AnalysisStopped';
import type { AssetAnalysisState } from '../bindings/AssetAnalysisState';
import type { Estimate } from '../bindings/Estimate';
import { call } from './call';

export type { UnlistenFn };

/** What Rust tells about a clip's analysis, each on its own event. */
export type AnalysisEvent =
  | { kind: 'progress'; payload: AnalysisProgress }
  | { kind: 'done'; payload: AnalysisDone }
  | { kind: 'failed'; payload: AnalysisFailed }
  | { kind: 'stopped'; payload: AnalysisStopped };

const EVENTS = {
  progress: 'analysis://progress',
  done: 'analysis://asset_done',
  failed: 'analysis://failed',
  stopped: 'analysis://stopped',
} as const;

/**
 * Analysis of the open project's clips (`crates/klipolo-app/src/commands/analysis.rs`), by asset id. A failure is an
 * Error with one plain sentence.
 */
export const analysisBridge = {
  /** What analysing these clips costs, which are cached, and with an access code the credit left and any refusal. */
  estimate: (assetIds: string[]) => call<Estimate>('analysis_estimate', { assetIds }),
  /**
   * Cached clips load at once; the others start, or the Error says why they can't. `byPage`: the page asks on its own
   * (loading cached clips), not the person, so rough words are timed again at most once per clip per app run.
   */
  start: (assetIds: string[], byPage = false) => call<void>('analysis_start', { assetIds, byPage }),
  /** Stops after the requests in flight; their clips go back to not analysed. */
  cancel: () => call<void>('analysis_cancel'),
  /** These clips left the project: they hear no more about their analysis. */
  forget: (assetIds: string[]) => call<void>('analysis_forget', { assetIds }),
  /** Every clip Rust knows of, and where its analysis is. */
  states: () => call<AssetAnalysisState[]>('analysis_states'),
  /** Every progress, done, failed and stopped event, until the returned function is called. */
  onEvent: async (handler: (event: AnalysisEvent) => void): Promise<UnlistenFn> => {
    const stops = await Promise.all(
      (Object.keys(EVENTS) as (keyof typeof EVENTS)[]).map((kind) =>
        listen(EVENTS[kind], (event) => handler({ kind, payload: event.payload } as AnalysisEvent)),
      ),
    );
    return () => stops.forEach((stop) => stop());
  },
};
