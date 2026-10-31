import type { Edge } from '../../../bindings/Edge';
import type { Item } from '../../../bindings/Item';
import type { Op } from '../../../bindings/Op';
import type { Project } from '../../../bindings/Project';
import { musicRunOf } from '../../../lib/musicRuns';
import { snapToFrame, US_PER_SEC } from '../../../lib/time';
import { findItem } from '../../../lib/timeline';
import { defaultTransitionUs, TRANSITION_DISSOLVE } from '../../../lib/transitions';
import { mainIndex } from '../model/index';
import { moveClipOps, moveItemOps } from '../model/moveOps';
import { trimClip, trimItemEdge } from '../model/trim';

/*
 * The keyboard's edits as Op batches: pure, built from the same model helpers a drag uses, so chat's tools can
 * make the same edits. Each returns no ops when it can't act (nothing there, a locked track, nowhere to go).
 */

/**
 * Q and W: `edge` of clip or layer `id` to the playhead, which must be inside it. A clip's trim is held to its
 * media and to one frame, and the model ripples what follows; a layer's is held to its media and its neighbours.
 */
export function trimToPlayheadOps(
  project: Project,
  id: string,
  playheadUs: number,
  edge: Edge,
  fps: number,
): Op[] {
  const at = snapToFrame(playheadUs, fps);
  const found = findItem(project, id);
  if (found) {
    const { item, track } = found;
    const end = item.startUs + item.durationUs;
    if (track.locked || at <= item.startUs || at >= end) return [];
    const atUs = trimItemEdge(project, track, item, edge, at, fps);
    return atUs === (edge === 'start' ? item.startUs : end) ? [] : [{ type: 'trimItem', id, edge, atUs }];
  }
  const main = project.timeline.main;
  const i = main.findIndex((clip) => clip.id === id);
  if (i < 0) return [];
  const { starts, ends } = mainIndex(main);
  if (at <= starts[i] || at >= ends[i]) return [];
  const from = edge === 'start' ? starts[i] : ends[i];
  const { sourceUs, deltaUs } = trimClip(project, main[i], edge, at - from, fps);
  return deltaUs ? [{ type: 'trimClip', id, edge, sourceUs }] : [];
}

/**
 * The , and . keys: layer `id` moves by `frames` on the frame grid, never before 0; clip `id` moves by `frames`
 * places on the main track (a press is one place, whatever ⇧ says), held to its ends.
 */
export function nudgeOps(project: Project, id: string, frames: number, fps: number): Op[] {
  const found = findItem(project, id);
  if (found) {
    const { item, track } = found;
    if (track.locked) return [];
    const frame = Math.round((item.startUs * fps) / US_PER_SEC) + frames;
    const startUs = Math.max(0, Math.round((frame * US_PER_SEC) / fps));
    return moveItemOps(item, track.id, track.id, startUs);
  }
  const main = project.timeline.main;
  const from = main.findIndex((clip) => clip.id === id);
  if (from < 0) return [];
  return moveClipOps(id, from, Math.min(main.length - 1, Math.max(0, from + frames)));
}

/**
 * `frames` held to where block `id` can go: a layer's start no earlier than 0:00, a clip between the first and the
 * last place. A held nudge adds up only what it can do, so a press back the other way moves at once.
 */
export function nudgeReach(project: Project, id: string, frames: number, fps: number): number {
  const found = findItem(project, id);
  if (found) return Math.max(frames, -Math.round((found.item.startUs * fps) / US_PER_SEC));
  const main = project.timeline.main;
  const from = main.findIndex((clip) => clip.id === id);
  return from < 0 ? 0 : Math.min(main.length - 1 - from, Math.max(-from, frames));
}

/**
 * ⌥↑ and ⌥↓: layer `itemId` to the nearest unlocked track of its kind above (`1`, a higher index, drawn higher) or
 * below (`-1`), at the same time. Where it overlaps a layer there, the model puts it on the next free track up.
 */
export function trackStepOps(project: Project, itemId: string, direction: 1 | -1): Op[] {
  const found = findItem(project, itemId);
  if (!found || found.track.locked) return [];
  const { tracks } = project.timeline;
  for (let t = tracks.indexOf(found.track) + direction; t >= 0 && t < tracks.length; t += direction) {
    const target = tracks[t];
    if (target.kind === found.track.kind && !target.locked)
      return [{ type: 'moveItem', id: itemId, trackId: target.id, startUs: found.item.startUs }];
  }
  return [];
}

/** Keep with clip / Free from clip: an attached layer lets go; a free one attaches to the clip under its start. */
export function attachOps(item: Item): Op[] {
  return [{ type: 'setAttach', id: item.id, attached: !item.attach }];
}

/**
 * ⌫ (and ⌘X after the copy): block `id` out of the video; a layer on a locked track stays. A song laid in pieces
 * back to back (`musicRunOf`) goes whole, in one step: no piece is left to start abruptly where the others stopped.
 */
export function deleteOps(project: Project, id: string): Op[] {
  const found = findItem(project, id);
  if (found) {
    if (found.track.locked) return [];
    const run = musicRunOf(project.timeline.tracks, id) ?? [found.item];
    return run.map((piece): Op => ({ type: 'removeItem', id: piece.id }));
  }
  return project.timeline.main.some((clip) => clip.id === id) ? [{ type: 'removeClip', id }] : [];
}

/**
 * ⌘T, a cut's Add transition: a dissolve from clip `clipIndex` into the next, half a second on the frame grid
 * or the longest that fits there. Nothing when no transition fits (no spare footage, or the last clip).
 */
export function addTransitionOps(project: Project, clipIndex: number, fps: number): Op[] {
  const clip = project.timeline.main[clipIndex];
  const durationUs = clip ? defaultTransitionUs(project, clipIndex, fps) : 0;
  if (durationUs === 0) return [];
  return [
    {
      type: 'setTransition',
      clipId: clip.id,
      transition: { effect: TRANSITION_DISSOLVE, durationUs, params: {} },
    },
  ];
}
