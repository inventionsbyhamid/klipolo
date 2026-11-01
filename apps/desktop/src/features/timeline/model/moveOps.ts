import type { Item } from '../../../bindings/Item';
import type { Op } from '../../../bindings/Op';
import type { Track } from '../../../bindings/Track';
import type { TrackKind } from '../../../bindings/TrackKind';

export function moveClipOps(clipId: string, from: number, to: number): Op[] {
  return from === to ? [] : [{ type: 'moveClip', id: clipId, index: to }];
}

export function moveItemOps(
  item: Item,
  fromTrackId: string,
  toTrackId: string,
  startUs: number,
  newTrack?: { id: string; kind: TrackKind; index: number },
): Op[] {
  if (newTrack)
    return [
      { type: 'addTrack', id: newTrack.id, kind: newTrack.kind, index: newTrack.index },
      { type: 'moveItem', id: item.id, trackId: newTrack.id, startUs },
    ];
  if (startUs === item.startUs && toTrackId === fromTrackId) return [];
  return [{ type: 'moveItem', id: item.id, trackId: toTrackId === fromTrackId ? null : toTrackId, startUs }];
}

const cache = new WeakMap<Track[], Map<string, string[]>>();

/** The layers attached to each clip (they follow it on screen while it's dragged). */
export function attachedTo(tracks: Track[]): Map<string, string[]> {
  let map = cache.get(tracks);
  if (!map) {
    map = new Map();
    for (const track of tracks)
      for (const item of track.items)
        if (item.attach) map.set(item.attach.clipId, [...(map.get(item.attach.clipId) ?? []), item.id]);
    cache.set(tracks, map);
  }
  return map;
}

/** The same ops, compared whole and deeply (a trim's ops differ only in their times, a patch's in nested fields). */
export function sameOps(a: readonly Op[], b: readonly Op[]): boolean {
  return a.length === b.length && a.every((op, i) => same(op, b[i]));
}

function same(a: unknown, b: unknown): boolean {
  if (a === b) return true;
  if (typeof a !== 'object' || typeof b !== 'object' || a === null || b === null) return false;
  if (Array.isArray(a) !== Array.isArray(b)) return false;
  const keys = Object.keys(a);
  if (keys.length !== Object.keys(b).length) return false;
  return keys.every((k) => same((a as Record<string, unknown>)[k], (b as Record<string, unknown>)[k]));
}
