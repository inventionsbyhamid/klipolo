import type { Asset } from '../../../bindings/Asset';
import type { Clip } from '../../../bindings/Clip';
import type { Item } from '../../../bindings/Item';
import type { Op } from '../../../bindings/Op';
import type { Project } from '../../../bindings/Project';
import type { TrackKind } from '../../../bindings/TrackKind';
import type { TrackTarget } from '../../../bindings/TrackTarget';
import { newClipFor, newId } from '../../../lib/defaults';
import { FORMAT_SIZE } from '../../../lib/layout';
import { audioItem, IMAGE_US, imageRect, layoutFor, placeable, videoLayerLayout } from '../../../lib/placement';
import { snapToFrame, US_PER_SEC } from '../../../lib/time';
import { itemSpans, mainIndex } from './index';

/**
 * Where media is put on the timeline: at cut `index` of the main track (0 before the first clip, `main.length` after
 * the last), or at `atUs` as a picture layer or a sound, on track `trackId` (null: wherever it fits, as the timeline's
 * lanes place them), or on a new track of its own (`newTrack`).
 */
export type DropTarget =
  | { lane: 'main'; index: number }
  | { lane: 'picture'; trackId: string | null; atUs: number; newTrack?: boolean }
  /**
   * `row`: the timeline lane it was held over, so only that lane shows where it lands. On Music it is added as music
   * that fits the video (`features/media/place`), never by `dropOps`.
   */
  | { lane: 'sound'; trackId: string | null; atUs: number; newTrack?: boolean; row?: 'sounds' | 'music' };

/** The cut of `main` nearest `atUs`: its index in the main track (`main.length` is the end). */
export function nearestCut(main: Clip[], atUs: number): number {
  const { starts, mainEndUs } = mainIndex(main);
  const n = starts.length;
  const cut = (i: number) => (i < n ? starts[i] : mainEndUs);
  // The first cut after `atUs` (the end, past the last clip's start), and the one before it.
  let lo = 0;
  let hi = n;
  while (lo < hi) {
    const mid = (lo + hi) >>> 1;
    if (starts[mid] <= atUs) lo = mid + 1;
    else hi = mid;
  }
  if (lo === 0) return 0;
  return atUs - cut(lo - 1) <= cut(lo) - atUs ? lo - 1 : lo;
}

/**
 * The ops that put `asset` at `target` (one undo step): the same builders the Import button uses (`lib/placement`).
 * The library and Finder drops send videos here (a clip of the main track, or a video layer); sounds and pictures go in
 * through Rust as the library's "+" places them (`features/media/place`). Its sound and picture layers are built as
 * before, for its own tests.
 *
 * - A video on the main track is a clip inserted at that cut.
 * - Anywhere else a video is a video layer 40 % of the frame in its bottom right corner (`videoLayerLayout`), and an
 *   image a 5 s layer, both attached to the clip under them; on the main track an image goes at the cut's time,
 *   exactly.
 * - A sound is a free audio layer, at the cut's time when dropped on the main track.
 * - A layer goes on the track it was dropped on when that track takes it (its kind, shown, unlocked), else wherever it
 *   fits: a video dropped on a sound lane is a picture layer.
 *
 * A layer starts where `layerStartUs` says: on the frame grid, or exactly on a cut or a layer's edge it was dropped on.
 * Media too short to play (under 10 ms) adds nothing.
 */
export function dropOps(project: Project, asset: Asset, target: DropTarget, fps: number): Op[] {
  if (!placeable(asset)) return [];
  const { main } = project.timeline;
  if (target.lane === 'main' && asset.kind === 'video') {
    const index = Math.max(0, Math.min(main.length, target.index));
    return [{ type: 'insertClip', clip: newClipFor(asset), index }];
  }
  const startUs =
    target.lane === 'main' ? cutUs(main, target.index) : layerStartUs(project, target.atUs, fps);
  return layerOps(project, asset, target, startUs);
}

/** Whether sorted `values` holds `value` exactly (binary search). */
function holds(values: Float64Array, value: number): boolean {
  let lo = 0;
  let hi = values.length;
  while (lo < hi) {
    const mid = (lo + hi) >>> 1;
    if (values[mid] < value) lo = mid + 1;
    else hi = mid;
  }
  return lo < values.length && values[lo] === value;
}

/**
 * Where a layer dropped at `atUs` starts. A cut within half a frame is taken exactly: imported clips end at their
 * file's own length, so most cuts are off the frame grid, and a start rounded off one would show the layer over the
 * clip before it for a frame and attach it to that clip. A time that is exactly a layer's edge (a drop snapped to it)
 * stays, so the layer meets that one without overlapping it; that holds for an edge just after a nearby cut too (the
 * cut would overlap the layer, so the engine would lift the new one a track up), but not for one just before it (the
 * new layer would show over the clip before the cut). Anything else goes on the frame grid. O(log cuts + tracks × log
 * layers), from the memoised spans.
 */
export function layerStartUs(project: Project, atUs: number, fps: number): number {
  const at = Math.max(0, atUs);
  const { main, tracks } = project.timeline;
  const cut = cutUs(main, nearestCut(main, at));
  const nearCut = fps > 0 && Math.abs(cut - at) <= US_PER_SEC / fps / 2;
  if (nearCut && at < cut) return cut;
  const onEdge = tracks.some((track) => {
    const { starts, ends } = itemSpans(track.items);
    return holds(starts, at) || holds(ends, at);
  });
  if (onEdge) return at;
  return nearCut ? cut : snapToFrame(at, fps);
}

/** A layer of `asset` from `startUs`, on the track `target` names when that track takes it, else wherever it fits. */
function layerOps(project: Project, asset: Asset, target: DropTarget, startUs: number): Op[] {
  const { tracks } = project.timeline;
  const kind: TrackKind = asset.kind === 'audio' ? 'audio' : 'overlay';
  const ops: Op[] = [];
  let track: TrackTarget = { type: 'auto' };
  if (target.lane === (kind === 'audio' ? 'sound' : 'picture')) {
    if (target.newTrack) {
      const id = newId();
      ops.push({ type: 'addTrack', id, kind, index: tracks.length });
      track = { type: 'track', id };
    } else if (target.trackId !== null) {
      // A hidden track would hide it, a locked one refuses it: as `auto` placement, neither is used.
      const on = tracks.find((t) => t.id === target.trackId);
      if (on && on.kind === kind && !on.locked && !on.hidden) track = { type: 'track', id: on.id };
    }
  }
  ops.push({ type: 'addItem', track, attach: kind !== 'audio', item: itemFor(project, asset, startUs) });
  return ops;
}

/** The time of cut `index`, exactly (the end past the last clip). */
function cutUs(main: Clip[], index: number): number {
  const { starts, mainEndUs } = mainIndex(main);
  return index >= 0 && index < starts.length ? starts[index] : index < 0 ? 0 : mainEndUs;
}

function itemFor(project: Project, asset: Asset, startUs: number): Item {
  if (asset.kind === 'audio') return { ...audioItem(asset), startUs };
  const base = { id: newId(), startUs, attach: null, effects: [] };
  if (asset.kind === 'image') {
    const [cw, ch] = FORMAT_SIZE[project.settings.primaryFormat];
    return {
      ...base,
      durationUs: IMAGE_US,
      layout: layoutFor(imageRect(asset, cw, ch)),
      content: { type: 'image', assetId: asset.id },
    };
  }
  return {
    ...base,
    durationUs: asset.durationUs,
    layout: videoLayerLayout(asset, project.settings.primaryFormat),
    content: { type: 'video', assetId: asset.id, sourceInUs: 0, speed: 1, volume: 1, muted: false },
  };
}

/**
 * Several files dropped at `target` at once (a Finder drop): `addAssets`, then each placed as `dropOps` would, in
 * order. Videos dropped on the main track go in one after another from that cut; each layer starts where the one
 * before it ends (and after a clip just inserted, at its end); a new track is made once and takes them all.
 */
export function dropAllOps(project: Project, assets: Asset[], target: DropTarget, fps: number): Op[] {
  const usable = assets.filter(placeable);
  if (usable.length === 0) return [];
  const ops: Op[] = [{ type: 'addAssets', assets: usable }];
  let now = project;
  let at = target;
  /** Where the next layer starts: where the last one placed ends (null: at the target). */
  let nextUs: number | null = null;
  for (const asset of usable) {
    // A layer after another starts exactly where that one ends (off the grid if it does): never overlapping it.
    let placed: Op[];
    if (nextUs !== null && !(asset.kind === 'video' && at.lane === 'main')) {
      const here: DropTarget =
        at.lane === 'main'
          ? { lane: asset.kind === 'audio' ? 'sound' : 'picture', trackId: null, atUs: nextUs }
          : { ...at, atUs: nextUs };
      placed = layerOps(now, asset, here, nextUs);
    } else placed = dropOps(now, asset, at, fps);
    ops.push(...placed);
    for (const op of placed) {
      if (op.type === 'insertClip' && at.lane === 'main') {
        const main = now.timeline.main;
        const index = op.index ?? main.length;
        now = {
          ...now,
          timeline: { ...now.timeline, main: [...main.slice(0, index), op.clip, ...main.slice(index)] },
        };
        at = { lane: 'main', index: index + 1 };
        nextUs = null;
      } else if (op.type === 'addTrack' && at.lane !== 'main') {
        at = { ...at, trackId: op.id, newTrack: false };
        now = {
          ...now,
          timeline: {
            ...now.timeline,
            tracks: [
              ...now.timeline.tracks,
              {
                id: op.id ?? '',
                kind: op.kind,
                name: '',
                hidden: false,
                muted: false,
                locked: false,
                items: [],
              },
            ],
          },
        };
      } else if (op.type === 'addItem') nextUs = op.item.startUs + op.item.durationUs;
    }
  }
  return ops;
}
