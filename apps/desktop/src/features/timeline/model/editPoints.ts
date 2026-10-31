import type { Project } from '../../../bindings/Project';
import { mainIndex } from './index';

const cache = new WeakMap<Project['timeline'], Float64Array>();

/** Where ↑ and ↓ stop: the start, every cut, every layer edge and the end, sorted, once each. */
export function editPoints(project: Project): Float64Array {
  let points = cache.get(project.timeline);
  if (!points) {
    const index = mainIndex(project.timeline.main);
    const all = [0, ...index.ends];
    for (const track of project.timeline.tracks)
      for (const item of track.items) all.push(item.startUs, item.startUs + item.durationUs);
    points = Float64Array.from(new Set(all)).sort();
    cache.set(project.timeline, points);
  }
  return points;
}

export function nextEdit(points: Float64Array, at: number): number | null {
  for (const p of points) if (p > at) return p;
  return null;
}

export function previousEdit(points: Float64Array, at: number): number | null {
  for (let i = points.length - 1; i >= 0; i--) if (points[i] < at) return points[i];
  return null;
}
