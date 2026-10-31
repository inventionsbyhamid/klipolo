import { t } from '@lingui/core/macro';
import { create } from 'zustand';
import type { AssetKind } from '../../bindings/AssetKind';
import { kindOfPath } from '../../lib/mediaKinds';
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
/** The timeline lane under a point: Overlays, Video, Sounds or Music; null off the lanes. */
export type Lane = 'layers' | 'main' | 'sounds' | 'music';
type LaneAt = (clientX: number, clientY: number) => Lane | null;

/** The timeline's point-to-target map and its lanes' scroller, while one is on screen (`TimelineView` registers it). */
let targetAt: TargetAt | null = null;
let laneAt: LaneAt | null = null;
let lanes: HTMLElement | null = null;

export function registerDropTarget(
  fn: TargetAt | null,
  scroller: HTMLElement | null = null,
  lane: LaneAt | null = null,
) {
  targetAt = fn;
  laneAt = fn ? lane : null;
  lanes = fn ? scroller : null;
}

/** What each lane takes, as the person hears it when something else is let go there. */
const goesOn = {
  audio: () => t({ id: 'drop.refused.sound', message: 'Sounds go on the Sounds or Music lane.' }),
  image: () => t({ id: 'drop.refused.picture', message: 'Pictures go on the Overlays lane.' }),
  video: () => t({ id: 'drop.refused.video', message: 'Videos go on the Video or Overlays lane.' }),
};

/**
 * Why media of `kind` let go at this point goes nowhere: it is over a lane that won't take it (a picture on the Video
 * or a sound lane, a sound on the Video or Overlays lane, a video on a sound lane). Null where it would land, off the
 * lanes, or when the kind isn't known.
 */
export function dropRefusal(clientX: number, clientY: number, kind: AssetKind | null): string | null {
  if (!kind || dropTargetAt(clientX, clientY, kind)) return null;
  const lane = laneAt?.(clientX, clientY) ?? null;
  return lane ? goesOn[kind]() : null;
}

/** What Finder is dragging, by the files' extensions: one kind when they are all of it, else null (any lane). */
export function kindOfPaths(paths: string[]): AssetKind | null {
  const kinds = new Set(paths.map(kindOfPath));
  return kinds.size === 1 ? [...kinds][0] : null;
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
  const row = (t: DropTarget) => (t.lane === 'sound' ? t.row : undefined);
  return a.lane === b.lane && a.trackId === b.trackId && a.atUs === b.atUs && row(a) === row(b);
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
 * Overlays lane a picture layer there; the Sounds lane a sound effect and the Music lane music. A layer's start keeps to the frame grid, and
 * edge or the playhead within 8 px while snapping is on: O(log edges) a frame.
 *
 * Null elsewhere: the ruler, outside the lanes, a read-only project, a zoom, hidden or locked track, and a row that
 * won't take what is dragged (`kind`: a sound over a picture row, a picture over a sound row, a sound or a picture over
 * the main track), so the indicator shows only where the media will really land (`dropRefusal` says why not).
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
  const picture = (): DropTarget | null =>
    kind === 'audio' ? null : { lane: 'picture', trackId: null, atUs: atUs() };
  const sound = (row: 'sounds' | 'music'): DropTarget | null =>
    kind === 'video' || kind === 'image' ? null : { lane: 'sound', trackId: null, atUs: atUs(), row };
  const within = (el: Element) => {
    const r = el.getBoundingClientRect();
    return clientY >= r.top && clientY < r.bottom;
  };
  for (const row of scroller.querySelectorAll<HTMLElement>('[data-lane]')) {
    if (!within(row)) continue;
    const lane = row.dataset.lane;
    // The main track takes videos (and files from Finder of mixed kinds); a sound or a picture goes on its own lane.
    if (lane === 'main')
      return kind === 'audio' || kind === 'image'
        ? null
        : { lane: 'main', index: nearestCut(project.timeline.main, pointerUs) };
    if (lane === 'layers') return picture();
    // Music and sound effects are both sound layers; what makes a song music (ducking) is set as it is added: a sound
    // let go on the Music lane is added as music (`row: 'music'`, the model's music ops), on the Sounds lane as an
    // effect.
    if (lane === 'sounds' || lane === 'music') return sound(lane);
  }
  return null;
}

/** The lane of `scroller` under this point, below its ruler (`headBottom`); null off the lanes. */
export function timelineLaneAt(
  scroller: HTMLElement,
  headBottom: number,
  clientX: number,
  clientY: number,
): Lane | null {
  const box = scroller.getBoundingClientRect();
  if (clientX < box.left || clientX >= box.right || clientY < Math.max(box.top, headBottom) || clientY >= box.bottom)
    return null;
  for (const row of scroller.querySelectorAll<HTMLElement>('[data-lane]')) {
    const r = row.getBoundingClientRect();
    if (clientY >= r.top && clientY < r.bottom) return (row.dataset.lane as Lane | undefined) ?? null;
  }
  return null;
}
