import type { Clip } from '../../../bindings/Clip';
import type { Item } from '../../../bindings/Item';
import type { Project } from '../../../bindings/Project';
import { audioLines } from '../lanes/audioRole';
import { mainIndex } from '../model/index';
import { packRows } from '../model/packRows';

const CUT = 'cut:';

/**
 * The cut after clip `clipId` on the Video lane, as the keyboard walks it (`cut:<clip id>`; model ids never contain
 * ':'): a focus target between two clips, never a selection. Its transition, if it has one, is what focus selects.
 */
export function cutId(clipId: string): string {
  return `${CUT}${clipId}`;
}

export function cutClipId(id: string | null | undefined): string | null {
  return id?.startsWith(CUT) ? id.slice(CUT.length) : null;
}

/** One lane as the keyboard walks it: its blocks' ids in time order, with where each starts and ends (µs). */
export interface Lane {
  id: string;
  ids: string[];
  starts: number[];
  ends: number[];
}

type Timeline = Project['timeline'];
const cachedLanes = new WeakMap<Timeline, Lane[]>();
const places = new WeakMap<Lane[], Map<string, readonly [number, number]>>();

function lane(id: string, items: readonly Item[]): Lane {
  return {
    id,
    ids: items.map((item) => item.id),
    starts: items.map((item) => item.startUs),
    ends: items.map((item) => item.startUs + item.durationUs),
  };
}

/**
 * The lanes top to bottom, as they draw (cached per timeline object): `layers` (the overlays the lane draws, by start;
 * those counted in a "+N" aren't drawn), `main`, then `sounds` and `music` (the sound layers of each, by start).
 * `expanded` is never on in Klipolo (every lane always shows); it is kept for the callers that pass the setting.
 */
export function laneOrder(project: Project, _expanded = false): Lane[] {
  const { timeline } = project;
  let lanes = cachedLanes.get(timeline);
  if (lanes) return lanes;
  const { main, tracks } = timeline;
  const byStart = (a: Item, b: Item) => a.startUs - b.startUs;
  const packing = packRows(tracks);
  const drawn = tracks.flatMap((t) => (t.kind === 'audio' ? [] : t.items.filter((i) => packing.row.has(i.id))));
  // Equal starts go top row first, as they draw.
  drawn.sort((a, b) => byStart(a, b) || packing.row.get(a.id)! - packing.row.get(b.id)!);
  const audio = audioLines(tracks);
  lanes = [lane('layers', drawn), mainLane(main), lane('sounds', audio.sounds.items), lane('music', audio.music.items)];
  cachedLanes.set(timeline, lanes);
  return lanes;
}

/** The Video lane: each clip, in order. */
function mainLane(main: Clip[]): Lane {
  const { starts, ends } = mainIndex(main);
  return { id: 'main', ids: main.map((c) => c.id), starts: [...starts], ends: [...ends] };
}

/** Where block `id` is: [lane, index], or null when no lane draws it. */
export function placeOf(lanes: Lane[], id: string): readonly [number, number] | null {
  let map = places.get(lanes);
  if (!map) {
    map = new Map();
    for (let l = 0; l < lanes.length; l++) lanes[l].ids.forEach((block, i) => map!.set(block, [l, i]));
    places.set(lanes, map);
  }
  return map.get(id) ?? null;
}

/**
 * What focus on block `id` selects: the block itself. (Klipolo draws no transitions, so the cuts between clips are no
 * stops of their own.)
 */
export function focusSelection(_project: Project, id: string): string | null {
  return id;
}

/** The block that stands for selection `id` in the lanes: itself. */
export function focusIdOf(id: string): string;
export function focusIdOf(id: string | null): string | null;
export function focusIdOf(id: string | null): string | null {
  return id;
}

/**
 * The block in `lane` that plays at `us` (the latest-starting one), else the one starting nearest it. Never a cut,
 * unless the lane has nothing else: ↑ / ↓ land on clips.
 */
function atTime(lane: Lane, us: number): number {
  let best = -1;
  let nearest = -1;
  for (let j = 0; j < lane.ids.length; j++) {
    if (cutClipId(lane.ids[j]) !== null) continue;
    if (lane.starts[j] <= us && us < lane.ends[j] && (best < 0 || lane.starts[j] >= lane.starts[best]))
      best = j;
    if (nearest < 0 || Math.abs(lane.starts[j] - us) < Math.abs(lane.starts[nearest] - us)) nearest = j;
  }
  return best >= 0 ? best : Math.max(0, nearest);
}

/* eslint-disable lingui/no-unlocalized-strings -- DOM key names */
/** The keys that move focus between blocks, by what they do. */
export const LANE_KEYS = {
  previous: 'ArrowLeft',
  next: 'ArrowRight',
  above: 'ArrowUp',
  below: 'ArrowDown',
  first: 'Home',
  last: 'End',
} as const;
/* eslint-enable lingui/no-unlocalized-strings */

export type LaneKey = (typeof LANE_KEYS)[keyof typeof LANE_KEYS];

/**
 * Where `key` takes focus from block `id`: ← / → the previous or next block in its lane; ↑ / ↓ the block in the
 * nearest lane above or below (empty lanes are skipped) at the same time; Home / End the lane's first or last. Null:
 * nowhere (the lane's end, the top or bottom lane).
 */
export function stepFrom(lanes: Lane[], id: string, key: LaneKey): string | null {
  const at = placeOf(lanes, id);
  if (!at) return null;
  const [l, i] = at;
  const { ids, starts } = lanes[l];
  if (key === 'ArrowLeft') return ids[i - 1] ?? null;
  if (key === 'ArrowRight') return ids[i + 1] ?? null;
  if (key === 'Home') return ids[0];
  if (key === 'End') return ids[ids.length - 1];
  const dir = key === 'ArrowUp' ? -1 : 1;
  for (let k = l + dir; k >= 0 && k < lanes.length; k += dir) {
    if (lanes[k].ids.length > 0) return lanes[k].ids[atTime(lanes[k], starts[i])];
  }
  return null;
}

/** The block of lane `laneId` at `us` (the latest one playing there, else the nearest start); null when it's empty. */
export function nearestIn(lanes: Lane[], laneId: string, us: number): string | null {
  const lane = lanes.find((l) => l.id === laneId);
  return lane && lane.ids.length > 0 ? lane.ids[atTime(lane, us)] : null;
}

/**
 * Where `key` takes focus from layer `id` starting at `startUs`, which lane `laneId` doesn't draw (compact view packed
 * it into "+N"): as if the lane drew it there.
 */
export function stepFromHidden(
  lanes: Lane[],
  laneId: string,
  id: string,
  startUs: number,
  key: LaneKey,
): string | null {
  const l = lanes.findIndex((lane) => lane.id === laneId);
  if (l < 0) return null;
  const { ids, starts, ends } = lanes[l];
  let i = 0;
  while (i < starts.length && starts[i] <= startUs) i++;
  const at = <T>(list: T[], value: T) => [...list.slice(0, i), value, ...list.slice(i)];
  const copy = lanes.slice();
  copy[l] = { id: laneId, ids: at(ids, id), starts: at(starts, startUs), ends: at(ends, startUs) };
  const to = stepFrom(copy, id, key);
  return to === id ? null : to;
}
