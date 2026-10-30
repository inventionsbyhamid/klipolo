import type { Item } from '../../../bindings/Item';
import type { Track } from '../../../bindings/Track';

/** The two audio lanes: background music, and sound effects (every other sound layer). */
export type AudioRole = 'music' | 'sounds';

/**
 * Whether `item` is music: a sound layer with ducking set (0 dB included), as the model's `is_music` says. Music is
 * always added with a duck, so turning ducking off keeps a song music; a sound effect has none.
 */
export function isMusic(item: Item): boolean {
  const content = item.content as { type: string; duck?: number | null };
  return content.type === 'audio' && typeof content.duck === 'number';
}

/** The lane a sound layer draws in. */
export function audioRole(item: Item): AudioRole {
  return isMusic(item) ? 'music' : 'sounds';
}

/** One audio track's layers that draw on a lane: still in start order and never overlapping, as on the track. */
export interface AudioLine {
  track: Track;
  items: Item[];
}

/** An audio lane: its tracks' lines, and every layer on it in start order (the keyboard walks those). */
export interface AudioLaneItems {
  lines: AudioLine[];
  items: Item[];
}

const split = new WeakMap<Track[], Record<AudioRole, AudioLaneItems>>();

/**
 * Every audio track's layers split into the Music and Sounds lanes. Built once per tracks array (an edit that leaves
 * the tracks alone keeps it), so each line's array keeps its identity and its spans stay cached.
 */
export function audioLines(tracks: Track[]): Record<AudioRole, AudioLaneItems> {
  let lanes = split.get(tracks);
  if (lanes) return lanes;
  lanes = { music: { lines: [], items: [] }, sounds: { lines: [], items: [] } };
  for (const track of tracks) {
    if (track.kind !== 'audio') continue;
    for (const role of ['sounds', 'music'] as const) {
      const items = track.items.filter((item) => audioRole(item) === role);
      if (items.length === 0) continue;
      lanes[role].lines.push({ track, items });
      lanes[role].items.push(...items);
    }
  }
  for (const lane of Object.values(lanes)) lane.items.sort((a, b) => a.startUs - b.startUs);
  split.set(tracks, lanes);
  return lanes;
}
