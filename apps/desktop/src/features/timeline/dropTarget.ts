import { create } from 'zustand';
import type { AssetKind } from '../../bindings/AssetKind';
import type { Project } from '../../bindings/Project';
import { snapToFrame } from '../../lib/time';
import { usePlayback } from '../../state/playback';
import { useProject } from '../../state/project';
import { useUi } from '../../state/ui';
import { startAutoscroll } from './drag/autoscroll';
import { SNAP_PX } from './drag/session';
import { HEADER_PX } from './layout';
import { nearestCut, type DropTarget } from './model/dropOps';
import { snapEdges, snapTargets } from './model/snap';
import { pxToUs } from './scale';

export type { DropTarget } from './model/dropOps';

/** The map from a point to a target; `kind`: what is dragged, when known (files in Finder aren't). */
type TargetAt = (clientX: number, clientY: number, kind?: AssetKind | null) => DropTarget | null;

/** The timeline's point-to-target map and its lanes' scroller, while one is on screen (`TimelineView` registers it). */
let targetAt: TargetAt | null = null;
let lanes: HTMLElement | null = null;

export function registerDropTarget(fn: TargetAt | null, scroller: HTMLElement | null = null) {
  targetAt = fn;
  lanes = fn ? scroller : null;
}

/**
 * Where media of `kind` let go at this point of the window (CSS px) would go; null: not over the timeline's lanes, or
 * over a row that won't take it.
 */
export function dropTargetAt(
  clientX: number,
  clientY: number,
  kind: AssetKind | null = null,
): DropTarget | null {
  return targetAt?.(clientX, clientY, kind) ?? null;
}

/**
 * While media is dragged: the lanes scroll when the pointer is over them within 32 px of their left or right edge
 * (`startAutoscroll`, as a block drag does), never while it is over the rest of the window or over the track-header
 * column. Media comes down from the side panel, which sits above that column: a block drag starts in the lanes and
 * may pull past their left edge, but a file brought in over the headers would scroll the lanes left by surprise.
 * `onScroll` runs in the frame the lanes moved. Returns the stop.
 */
export function dropAutoscroll(pointer: () => { x: number; y: number }, onScroll: () => void): () => void {
  const scroller = lanes;
  if (!scroller) return () => undefined;
  // The lanes don't move during a drag: one layout read.
  const box = scroller.getBoundingClientRect();
  const middle = box.left + box.width / 2;
  const lanesLeft = box.left + HEADER_PX;
  return startAutoscroll(
    scroller,
    HEADER_PX,
    () => {
      const { x, y } = pointer();
      return y >= box.top && y < box.bottom && x >= lanesLeft ? x : middle;
    },
    onScroll,
  );
}

/**
 * Where media being dragged (from the media panel or Finder) would land, for `DropIndicator`, and how long it is
 * (null: not known, as for files still in Finder).
 */
export const useDropIndicator = create<{ target: DropTarget | null; lengthUs: number | null }>(() => ({
  target: null,
  lengthUs: null,
}));

function sameTarget(a: DropTarget | null, b: DropTarget | null): boolean {
  if (a === null || b === null) return a === b;
  if (a.lane === 'main' || b.lane === 'main')
    return a.lane === 'main' && b.lane === 'main' && a.index === b.index;
  return a.lane === b.lane && a.trackId === b.trackId && a.atUs === b.atUs && !a.newTrack === !b.newTrack;
}

/** Shows where a drop would land (null hides it). The same target again changes nothing, so nothing re-renders. */
export function setDropIndicator(target: DropTarget | null, lengthUs: number | null = null) {
  const now = useDropIndicator.getState();
  if (sameTarget(now.target, target) && now.lengthUs === lengthUs) return;
  useDropIndicator.setState({ target, lengthUs });
}

/**
 * Each project's snap edges (the start, every cut and every layer's edges, sorted): built once per revision, not per
 * frame, and let go with the project (about 17 KB for an hour of 1,500 cuts and 300 titles). The playhead, which moves
 * during playback, is checked on its own.
 */
const edges = new WeakMap<Project, Float64Array>();

function edgesOf(project: Project): Float64Array {
  let built = edges.get(project);
  if (!built) edges.set(project, (built = snapTargets(project, 0, new Set())));
  return built;
}

/**
 * Over `scroller`'s lanes (below its ruler, `headBottom`): the main lane gives the cut nearest the pointer's time; the
 * Overlays lane a picture layer there; the Sounds and Music lanes a sound. A layer's start keeps to the frame grid, and
 * edge or the playhead within 8 px while snapping is on: O(log edges) a frame.
 *
 * Null elsewhere: the ruler, outside the lanes, a read-only project, a zoom, hidden or locked track, and a row that
 * won't take what is dragged (`kind`: a sound over a picture row, a picture over a sound row), so the indicator shows
 * only where the media will really land. The main track takes every kind.
 */
export function timelineTargetAt(
  scroller: HTMLElement,
  headBottom: number,
  clientX: number,
  clientY: number,
  kind: AssetKind | null = null,
): DropTarget | null {
  const snapshot = useProject.getState().snapshot;
  if (!snapshot || snapshot.readOnly) return null;
  const box = scroller.getBoundingClientRect();
  if (
    clientX < box.left ||
    clientX >= box.right ||
    clientY < Math.max(box.top, headBottom) ||
    clientY >= box.bottom
  )
    return null;
  const { project } = snapshot;
  const { pxPerSecond: pps, snapping } = useUi.getState();
  // Over the track headers: the lanes' left edge.
  const px = Math.max(0, clientX - box.left - HEADER_PX) + scroller.scrollLeft;
  const pointerUs = Math.max(0, pxToUs(px, pps));
  const atUs = () => {
    const at = snapToFrame(pointerUs, project.settings.fps);
    if (!snapping) return at;
    const reach = pxToUs(SNAP_PX, pps);
    const hit = snapEdges([at], edgesOf(project), reach);
    const playheadUs = usePlayback.getState().playheadUs;
    const toPlayhead = Math.abs(playheadUs - at);
    if (toPlayhead <= reach && (!hit || toPlayhead < Math.abs(hit.deltaUs))) return playheadUs;
    return hit ? hit.atUs : at;
  };
  const picture = (trackId: string | null, newTrack?: boolean): DropTarget | null =>
    kind === 'audio' ? null : { lane: 'picture', trackId, atUs: atUs(), ...(newTrack ? { newTrack } : {}) };
  const sound = (trackId: string | null, newTrack?: boolean): DropTarget | null =>
    kind === 'video' || kind === 'image'
      ? null
      : { lane: 'sound', trackId, atUs: atUs(), ...(newTrack ? { newTrack } : {}) };
  const within = (el: Element) => {
    const r = el.getBoundingClientRect();
    return clientY >= r.top && clientY < r.bottom;
  };
  const zone = scroller.querySelector<HTMLElement>('[data-new-track]');
  if (zone && within(zone))
    return zone.dataset.newTrack === 'audio' ? sound(null, true) : picture(null, true);
  for (const row of scroller.querySelectorAll<HTMLElement>('[data-lane]')) {
    if (!within(row)) continue;
    const lane = row.dataset.lane;
    if (lane === 'main') return { lane: 'main', index: nearestCut(project.timeline.main, pointerUs) };
    if (lane === 'layers') return picture(null);
    // Music and sound effects are both sound layers; what makes a song music (ducking) is set as it is added.
    if (lane === 'sounds' || lane === 'music') return sound(null);
    const track = project.timeline.tracks.find((t) => t.id === row.dataset.trackId);
    // A zoom track takes no media; a hidden one would hide it, a locked one takes nothing.
    if (!track || track.locked || track.hidden || track.kind === 'zoom') return null;
    return track.kind === 'audio' ? sound(track.id) : picture(track.id);
  }
  return null;
}
