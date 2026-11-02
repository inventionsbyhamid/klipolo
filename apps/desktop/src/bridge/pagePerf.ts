import { call } from './call';

/** The page's perf readout (`perf/readout.ts`): its lines go into the app's log, which ignores them unless it is on. */
export const pagePerf = {
  log: (line: string) => call<void>('page_perf_log', { line }),
};
