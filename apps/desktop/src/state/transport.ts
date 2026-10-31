import { videoDurationUs } from '../lib/timeline';
import { clockUs } from './playClock';
import { usePlayback } from './playback';
import { useProject } from './project';

/** The video's length in µs (0 with no project open). */
export function useDurationUs(): number {
  return useProject((s) => (s.snapshot ? videoDurationUs(s.snapshot.project.timeline) : 0));
}

/** Moves the playhead by `deltaUs` from where it is now (the clock's place while playing), within the video. */
export function skip(deltaUs: number, durationUs: number) {
  const { playing, playheadUs, seek } = usePlayback.getState();
  const at = playing ? clockUs() : playheadUs;
  seek(Math.min(Math.max(at + deltaUs, 0), durationUs));
}

/**
 * Keeps the paused playhead inside the video as edits change its length: an undo or a delete that makes it shorter
 * than where the playhead was brings the playhead to its new end. Returns the stop.
 */
export function keepPlayheadInVideo(): () => void {
  return useProject.subscribe((s, prev) => {
    if (!s.snapshot || s.snapshot === prev.snapshot) return;
    const durationUs = videoDurationUs(s.snapshot.project.timeline);
    const { playing, playheadUs, seek } = usePlayback.getState();
    if (!playing && playheadUs > durationUs) seek(durationUs);
  });
}

/** Where the playhead was when the page went away (a reload in development, a WebKit content restart). */
const KEPT = 'klipolo.playhead';

/**
 * Keeps the playhead's place across a reload of the page: it is noted as the page goes (`pagehide`), with the project
 * it was in, and `restorePlayhead` takes it back once that project is open again. Nothing is kept across projects or
 * past the session (sessionStorage). Returns the stop.
 */
export function keepPlayheadOnReload(): () => void {
  const note = () => {
    const project = useProject.getState().snapshot?.project.id;
    if (!project) return;
    try {
      sessionStorage.setItem(KEPT, JSON.stringify({ project, us: usePlayback.getState().playheadUs }));
    } catch {
      // Storage refused (private mode, quota): the playhead starts at 0, as before.
    }
  };
  window.addEventListener('pagehide', note);
  return () => window.removeEventListener('pagehide', note);
}

/** The playhead back where it was before the page reloaded, when the same project is open (within its video). */
export function restorePlayhead() {
  const kept = takeKept();
  const snapshot = useProject.getState().snapshot;
  if (!kept || !snapshot || kept.project !== snapshot.project.id || typeof kept.us !== 'number') return;
  const durationUs = videoDurationUs(snapshot.project.timeline);
  usePlayback.getState().seek(Math.min(Math.max(kept.us, 0), durationUs));
}

/** What `keepPlayheadOnReload` noted, taken once (null when nothing was, or storage is refused). */
function takeKept(): { project?: string; us?: number } | null {
  try {
    const kept = JSON.parse(sessionStorage.getItem(KEPT) ?? 'null') as { project?: string; us?: number } | null;
    sessionStorage.removeItem(KEPT);
    return kept;
  } catch {
    return null;
  }
}
