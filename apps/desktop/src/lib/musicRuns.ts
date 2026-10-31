import type { Item } from '../bindings/Item';
import type { Track } from '../bindings/Track';

/** How far apart two pieces may be and still play back to back (the model's `OVERLAP_TOLERANCE_US`). */
const TOUCH_US = 4;

/** Whether `item` can be a piece of a run: free music (ducking set, 0 dB included) of a file without clock drift. */
function isPiece(item: Item): boolean {
  const c = item.content;
  return c.type === 'audio' && typeof c.duck === 'number' && !item.attach && (c.driftPpm ?? 0) === 0;
}

const fileOf = (item: Item) => (item.content.type === 'audio' ? item.content.assetId : null);

/**
 * The runs of music pieces on `track`, each one song laid back to back: free music of one file without clock drift,
 * each piece starting where the one before ends. `[first, end)` indexes into `track.items`, in order. Mirrors the
 * model's `media::music::runs` (pinned by `music-runs-parity.json`), so the page edits as one song what the refit fits
 * as one.
 */
export function musicRuns(track: Track): [number, number][] {
  const out: [number, number][] = [];
  track.items.forEach((item, i) => {
    if (!isPiece(item)) return;
    const run = out[out.length - 1];
    const before = run && run[1] === i ? track.items[i - 1] : null;
    const follows =
      before !== null &&
      fileOf(before) === fileOf(item) &&
      Math.abs(item.startUs - (before.startUs + before.durationUs)) <= TOUCH_US;
    if (follows) run[1] = i + 1;
    else out.push([i, i + 1]);
  });
  return out;
}

/** Each track's runs, worked out once per tracks array (a patch keeps it while no layer changes). */
const byTracks = new WeakMap<Track[], Map<string, Item[]>>();

/**
 * The run music piece `id` is in, as `tracks` have it: its pieces in order (one, for music laid once). Null when `id`
 * is no music piece (a sound effect, attached or drifting music, another kind of layer) or isn't there.
 */
export function musicRunOf(tracks: Track[], id: string): Item[] | null {
  let runs = byTracks.get(tracks);
  if (!runs) {
    runs = new Map();
    for (const track of tracks) {
      if (track.kind !== 'audio') continue;
      for (const [first, end] of musicRuns(track)) {
        const pieces = track.items.slice(first, end);
        for (const piece of pieces) runs.set(piece.id, pieces);
      }
    }
    byTracks.set(tracks, runs);
  }
  return runs.get(id) ?? null;
}
