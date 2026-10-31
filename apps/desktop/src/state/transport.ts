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
