import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type { MediaJobList } from '../bindings/MediaJobList';
import type { PauseBy } from '../bindings/PauseBy';
import { call } from './call';

/**
 * Editing copies (`crates/klipolo-app/src/media_jobs`): lighter copies of the open project's slow files, made one at a
 * time for the preview (export always reads the files themselves). Rust lists them in the order they are made and
 * sends the whole list again on every change (`media-jobs`, progress at most 4 times a second), with who holds them
 * (`held`).
 */
export const mediaJobs = {
  /** The open project's copies: waiting, being made, paused, done, failed or stopped. */
  list: () => call<MediaJobList>('media_jobs_list'),
  /**
   * Holds every copy (the running one keeps its finished part) until the same side resumes. `by` says who: the
   * person (the top bar's pill) or an action; one never undoes the other's pause.
   */
  pause: (by: PauseBy) => call<void>('media_jobs_pause', { by }),
  /** Undoes `by`'s own pause. The person's also restarts copies paused for a full disk, heat or a busy encoder. */
  resume: (by: PauseBy) => call<void>('media_jobs_resume', { by }),
  /** Stops `path`'s copy and deletes what it made so far; a finished copy stays. */
  stop: (path: string) => call<void>('media_job_stop', { path }),
  /** Makes `path`'s copy again: a failed, stopped or paused one waits its turn. */
  retry: (path: string) => call<void>('media_job_retry', { path }),
  /** Whether copies are made by themselves. Off: unfinished ones are forgotten; finished ones stay in use. */
  setAuto: (auto: boolean) => call<void>('media_jobs_set_auto', { auto }),
  /**
   * The whole list again, on every change. Stopping never fails: where Tauri's events can't be removed (the page's
   * tests, a window closing) there is nothing left to stop.
   */
  onChange: async (handler: (list: MediaJobList) => void): Promise<UnlistenFn> => {
    const off = await listen<MediaJobList>('media-jobs', (event) => handler(event.payload));
    return () => void Promise.resolve(off()).catch(() => undefined);
  },
};
