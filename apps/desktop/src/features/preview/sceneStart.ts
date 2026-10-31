import type { Project } from '../../bindings/Project';
import { clipIndexAt, itemSpans, mainIndex } from '../timeline/model/index';

/** The last index whose start is at or before `at`, or −1: binary search over sorted starts. */
function lastStartAtOrBefore(starts: Float64Array, at: number): number {
  let lo = 0;
  let hi = starts.length;
  while (lo < hi) {
    const mid = (lo + hi) >>> 1;
    if (starts[mid] <= at) lo = mid + 1;
    else hi = mid;
  }
  return lo - 1;
}

/**
 * When what the canvas shows at `at` began: the latest clip start, layer start or layer end at or before it. The
 * clip and the visible layers are the same anywhere from here to `at`, so views that draw "what is at the playhead"
 * key on this and re-render only when a layer enters or leaves, not 30 times a second while playing.
 *
 * It runs on every playback-store update (each frame while playing, each pointer move while scrubbing), so it reads the
 * memoised spans (`mainIndex`, `itemSpans`: built once per array, which a patch keeps when unchanged) instead of
 * walking every clip and layer. A track's layers are sorted and never overlap, so its ends are sorted too: the last
 * layer starting at or before `at` either holds `at` (its start counts) or has the latest end before it.
 * O(log clips + overlay tracks × log layers) per call, no allocation once the spans exist; it was O(clips + layers).
 */
export function sceneStartUs(project: Project, at: number): number {
  const index = mainIndex(project.timeline.main);
  const clip = clipIndexAt(index, at);
  let start = clip >= 0 ? index.starts[clip] : 0;
  for (const track of project.timeline.tracks) {
    if (track.kind !== 'overlay' || track.hidden || track.items.length === 0) continue;
    const { starts, ends } = itemSpans(track.items);
    const i = lastStartAtOrBefore(starts, at);
    if (i < 0) continue;
    const edge = at < ends[i] ? starts[i] : ends[i];
    if (edge > start) start = edge;
  }
  return start;
}
