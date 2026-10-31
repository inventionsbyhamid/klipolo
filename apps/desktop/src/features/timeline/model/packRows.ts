import type { Item } from '../../../bindings/Item';
import type { Track } from '../../../bindings/Track';

export const MAX_ROWS = 3;
/** Overlaps this small count as touching (the model's tolerance). */
const TOUCH_US = 4;

/**
 * One packed row: its layers in start order (they never overlap), and the track each is on (`tracks[i]` holds
 * `items[i]`).
 */
export interface PackedRow {
  items: Item[];
  tracks: Track[];
}

export interface Packing {
  row: Map<string, number>;
  rows: number;
  /** Each row's layers. Sub-pixel runs merge per row, so a band never crosses rows or covers a block. */
  lines: PackedRow[];
  /**
   * Layers that found no row, in runs of overlapping ones (start order, never overlapping each other): the lane shows
   * "+N" over each run, even when the run started left of the window. `items` are the run's layers.
   */
  overflow: { startUs: number; endUs: number; count: number; items: Item[] }[];
}

const packs = new WeakMap<Track[], Packing>();

/**
 * Compact mode's Layers lane: every layer of every overlay and zoom track, in start order (the top track
 * first at equal starts), each in the first of three rows where it doesn't overlap.
 */
export function packRows(tracks: Track[]): Packing {
  const cached = packs.get(tracks);
  if (cached) return cached;
  const items = tracks
    .flatMap((track, t) => (track.kind === 'audio' ? [] : track.items.map((item) => ({ item, t }))))
    .sort((a, b) => a.item.startUs - b.item.startUs || b.t - a.t);
  const ends: number[] = [];
  const row = new Map<string, number>();
  const lines: PackedRow[] = [];
  const overflow: Packing['overflow'] = [];
  for (const { item, t } of items) {
    const r = ends.findIndex((end) => end <= item.startUs + TOUCH_US);
    const at = r >= 0 ? r : ends.length;
    const endUs = item.startUs + item.durationUs;
    if (at < MAX_ROWS) {
      ends[at] = endUs;
      row.set(item.id, at);
      lines[at] ??= { items: [], tracks: [] };
      lines[at].items.push(item);
      lines[at].tracks.push(tracks[t]);
      continue;
    }
    const run = overflow.at(-1);
    if (run && item.startUs < run.endUs) {
      run.count++;
      run.endUs = Math.max(run.endUs, endUs);
      run.items.push(item);
    } else overflow.push({ startUs: item.startUs, endUs, count: 1, items: [item] });
  }
  const packing: Packing = { row, rows: Math.max(1, Math.min(ends.length, MAX_ROWS)), lines, overflow };
  packs.set(tracks, packing);
  return packing;
}
