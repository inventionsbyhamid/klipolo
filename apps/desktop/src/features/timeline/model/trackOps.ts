import type { Op } from '../../../bindings/Op';
import type { Track } from '../../../bindings/Track';
import type { TrackKind } from '../../../bindings/TrackKind';
import { soloable, soloedTracks } from '../../../lib/solo';

/*
 * The track edits as ops: the timeline's headers, menus and commands build them here, and chat's tools can too.
 */

/** The longest track name the header takes. */
export const TRACK_NAME_MAX = 100;

/** A new, empty track of `kind`, named and placed by the model (an overlay on top, audio at the bottom). */
export function addTrackOps(kind: TrackKind): Op[] {
  return [{ type: 'addTrack', id: null, kind, index: null }];
}

/** Track `track` renamed to `name` (trimmed): nothing when that is empty or the name it already has. */
export function renameTrackOps(track: Track, name: string): Op[] {
  const next = name.trim().slice(0, TRACK_NAME_MAX);
  return next === '' || next === track.name
    ? []
    : [{ type: 'updateTrack', id: track.id, patch: { name: next } }];
}

const visual = (track: Track) => track.kind !== 'audio';

/**
 * Where track `id` goes to move one row up (1) or down (−1) among its own rows: the index of the next picture track
 * above or below it (they draw last on top, above Video), or audio track (first on top, below Video). Null at an end.
 */
export function trackStepIndex(tracks: Track[], id: string, direction: 1 | -1): number | null {
  const i = tracks.findIndex((track) => track.id === id);
  if (i < 0) return null;
  const group = visual(tracks[i]);
  const step = group ? direction : -direction;
  for (let j = i + step; j >= 0 && j < tracks.length; j += step) if (visual(tracks[j]) === group) return j;
  return null;
}

/** Track `id` one row up (1) or down (−1): it takes its neighbour's index (the model moves it there). */
export function moveTrackOps(tracks: Track[], id: string, direction: 1 | -1): Op[] {
  const index = trackStepIndex(tracks, id, direction);
  return index === null ? [] : [{ type: 'moveTrack', id, index }];
}

export type TrackToggle = 'hidden' | 'muted' | 'locked' | 'solo';

/**
 * Whether `field` means anything on `track`: picture tracks hide, audio tracks mute, overlay and audio tracks solo (the
 * model refuses a zoom track's), every track locks.
 */
export function trackHas(track: Pick<Track, 'kind'>, field: TrackToggle): boolean {
  if (field === 'solo') return soloable(track);
  return field === 'locked' || (field === 'muted') === (track.kind === 'audio');
}

/** What `track`'s `field` is (solo, which Rust leaves out when it's off, reads false). */
export const trackFlag = (track: Track, field: TrackToggle): boolean => track[field] ?? false;

/** The toggles a track's header and menu show, in order: hide (or mute), solo where it can, lock. */
export function trackToggles(track: Pick<Track, 'kind'>): TrackToggle[] {
  const toggles: TrackToggle[] = [track.kind === 'audio' ? 'muted' : 'hidden', 'solo', 'locked'];
  return toggles.filter((field) => trackHas(track, field));
}

/**
 * `track` hidden or shown, muted or unmuted, soloed or not, locked or unlocked: `value` is the state it ends in, so
 * asking twice (chat's "mute the music") never undoes the first. Nothing when it is already so, or `field` doesn't
 * apply to its kind.
 */
export function setTrackOps(track: Track, field: TrackToggle, value: boolean): Op[] {
  if (!trackHas(track, field) || trackFlag(track, field) === value) return [];
  return [{ type: 'updateTrack', id: track.id, patch: { [field]: value } }];
}

/** Solo off on every soloed track, in one change (the export's "Export everything", chat's "unsolo everything"). */
export function soloOffOps(tracks: readonly Track[]): Op[] {
  return soloedTracks(tracks).map((track) => ({ type: 'updateTrack', id: track.id, patch: { solo: false } }));
}

/**
 * Track `track` out of the project, with its layers (a removal: one undo step brings both back). A locked one stays.
 */
export function removeTrackOps(track: Track): Op[] {
  return track.locked ? [] : [{ type: 'removeTrack', id: track.id }];
}
