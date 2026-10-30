import type { Track } from '../bindings/Track';

/**
 * What the tracks' solo buttons leave on, as `klipolo_model::timeline::Solo` resolves it (non-destructive: each track's
 * own hide and mute are untouched). The main track always plays and shows. O(tracks) to resolve.
 */
export interface Solo {
  /** Some overlay or audio track is soloed: only soloed tracks are heard. */
  sound: boolean;
  /** Some overlay track is soloed: only soloed overlay tracks are seen. */
  picture: boolean;
}

/** Whether `track` can be soloed: overlay and audio tracks (the model refuses a zoom track). */
export const soloable = (track: Pick<Track, 'kind'>) => track.kind !== 'zoom';

export function soloOf(tracks: readonly Track[]): Solo {
  const solo: Solo = { sound: false, picture: false };
  for (const track of tracks) {
    if (!track.solo || !soloable(track)) continue;
    solo.sound = true;
    solo.picture ||= track.kind === 'overlay';
  }
  return solo;
}

/** Whether `track`'s pictures are drawn: an overlay track that isn't hidden, and soloed while any overlay track is. */
export function showsPictures(solo: Solo, track: Track): boolean {
  return track.kind === 'overlay' && !track.hidden && (!!track.solo || !solo.picture);
}

/** The soloed tracks, in the project's order (what an export keeps while solo is on). */
export function soloedTracks(tracks: readonly Track[]): Track[] {
  return tracks.filter((track) => track.solo && soloable(track));
}
