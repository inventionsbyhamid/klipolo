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
