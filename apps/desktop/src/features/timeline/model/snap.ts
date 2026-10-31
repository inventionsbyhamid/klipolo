import type { Project } from '../../../bindings/Project';
import { mainIndex } from './index';

/**
 * What a dragged edge holds to: the start, the end, every cut, every other layer's edges and the playhead.
 * With `rippleAfter`, a trim of that clip's end slides what comes after it: the later cuts, the end and the layers
 * attached to later clips move with the edge, so they aren't targets.
 */
export function snapTargets(
  project: Project,
  playheadUs: number,
  exclude: ReadonlySet<string>,
  rippleAfter?: number,
): Float64Array {
  const { main } = project.timeline;
  const index = mainIndex(main);
  // The last cut is the end.
  const out: number[] = [0, playheadUs, ...index.ends.subarray(0, (rippleAfter ?? main.length) + 1)];
  const moving = rippleAfter === undefined ? null : new Set(main.slice(rippleAfter + 1).map((c) => c.id));
  for (const track of project.timeline.tracks)
    for (const item of track.items)
      if (!exclude.has(item.id) && !(item.attach && moving?.has(item.attach.clipId)))
        out.push(item.startUs, item.startUs + item.durationUs);
  return Float64Array.from(out).sort();
}

function nearest(targets: Float64Array, t: number): number {
  let lo = 0;
  let hi = targets.length;
  while (lo < hi) {
    const mid = (lo + hi) >>> 1;
    if (targets[mid] < t) lo = mid + 1;
    else hi = mid;
  }
  const after = lo < targets.length ? targets[lo] : Infinity;
  const before = lo > 0 ? targets[lo - 1] : -Infinity;
  return t - before <= after - t ? before : after;
}

/** The smallest move within `thresholdUs` that puts one of `edges` on a target; null when none is that near. */
export function snapEdges(
  edges: readonly number[],
  targets: Float64Array,
  thresholdUs: number,
): { deltaUs: number; atUs: number } | null {
  let best: { deltaUs: number; atUs: number } | null = null;
  for (const edge of edges) {
    const at = nearest(targets, edge);
    const deltaUs = at - edge;
    if (Math.abs(deltaUs) <= thresholdUs && (!best || Math.abs(deltaUs) < Math.abs(best.deltaUs))) {
      best = { deltaUs, atUs: at };
    }
  }
  return best;
}
