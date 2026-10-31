import { describe, expect, it } from 'vitest';
import type { Project } from '../../bindings/Project';
import { clipAt, itemEndUs } from '../../lib/timeline';
import { fixtureProject } from '../../test/fixtures';
import { longProject } from '../../test/longProject';
import { sceneStartUs } from './sceneStart';

const S = 1_000_000;

/** The walk it replaces (O(clips + layers) per playback update), as the reference. */
function walked(project: Project, at: number): number {
  let start = clipAt(project.timeline.main, at)?.startUs ?? 0;
  for (const track of project.timeline.tracks) {
    if (track.kind !== 'overlay' || track.hidden) continue;
    for (const item of track.items) {
      const end = itemEndUs(item);
      if (item.startUs <= at && at < end) start = Math.max(start, item.startUs);
      else if (end <= at) start = Math.max(start, end);
    }
  }
  return start;
}

/** Real scale: two hours of 1,500 cuts at 4.8 s, and 300 titles on three tracks. */
function twoHours(): Project {
  const p = longProject();
  p.timeline.main = p.timeline.main.map((clip, i) => ({
    ...clip,
    sourceInUs: Math.round(i * 2.4 * S),
    sourceOutUs: Math.round(i * 2.4 * S + 4.8 * S),
  }));
  for (const track of p.timeline.tracks)
    track.items = track.items.map((item) => ({ ...item, startUs: item.startUs * 2, durationUs: 8 * S }));
  return p;
}

describe('when the scene at the playhead began', () => {
  it('matches the full walk everywhere: inside layers, on their edges, between them, past the end', () => {
    const fixture = fixtureProject();
    const hidden = fixtureProject();
    hidden.timeline.tracks[0].hidden = true;
    for (const project of [fixture, hidden, longProject(), twoHours()]) {
      const edges = project.timeline.tracks.flatMap((t) => t.items.flatMap((i) => [i.startUs, itemEndUs(i)]));
      const times = [0, 1, ...edges, ...edges.map((e) => e - 1), ...edges.map((e) => e + 1)];
      for (let at = 0; at < 7300 * S; at += 997_331) times.push(at);
      for (const at of times) expect(sceneStartUs(project, at)).toBe(walked(project, at));
    }
  });

  it('costs a few binary searches a playback update at real scale (two hours, 1,500 cuts, 300 layers)', () => {
    const project = twoHours();
    const frames = 20_000;
    const step = (7200 * S) / frames;
    sceneStartUs(project, 0);
    let sum = 0;
    const began = performance.now();
    for (let i = 0; i < frames; i++) sum += sceneStartUs(project, i * step);
    const perUpdateUs = ((performance.now() - began) * 1000) / frames;
    expect(sum).toBeGreaterThan(0);
    // The target: under 5 µs an update, from 25 to 150 µs for the walk.
    expect(perUpdateUs).toBeLessThan(5);
  });
});
