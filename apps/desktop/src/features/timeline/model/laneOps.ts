import type { Op } from '../../../bindings/Op';
import type { Track } from '../../../bindings/Track';
import { isMusic } from '../lanes/audioRole';
import { setTrackOps, trackFlag, trackHas, type TrackToggle } from './trackOps';

/*
 * A lane's toggles as ops: the header and options of Overlays, Sound effects and Music act on every track the lane
 * draws, in one change, as the track edits do on one track (`trackOps`).
 */

/** The lanes with tracks of their own: Overlays (picture tracks), Sound effects and Music (audio tracks). */
export type LaneId = 'layers' | 'sounds' | 'music';

/**
 * The tracks lane `lane` draws, in the project's order: every overlay and zoom track for Overlays; for Sound effects
 * and Music, each audio track holding a layer of that kind (a sound effect never goes onto a track that holds music,
 * so a track is on both lanes only when music was added to a track of sound effects).
 */
export function laneTracks(tracks: readonly Track[], lane: LaneId): Track[] {
  if (lane === 'layers') return tracks.filter((track) => track.kind !== 'audio');
  const music = lane === 'music';
  return tracks.filter(
    (track) =>
      track.kind === 'audio' &&
      track.items.some((item) => item.content.type === 'audio' && isMusic(item) === music),
  );
}

/** What a lane's Hide or Mute is: Overlays hide, the sound lanes mute. */
export const laneOff = (lane: LaneId): TrackToggle => (lane === 'layers' ? 'hidden' : 'muted');

/** The toggles a lane's options list, in order: Hide (or Mute), Solo where one of its tracks can, Lock. */
export function laneToggles(lane: LaneId, on: readonly Track[]): TrackToggle[] {
  const toggles: TrackToggle[] = [laneOff(lane), 'solo', 'locked'];
  return toggles.filter((field) => on.some((track) => trackHas(track, field)));
}

/** Whether every track of the lane that `field` applies to has it: the lane is hidden, muted, soloed or locked. */
export function laneFlag(on: readonly Track[], field: TrackToggle): boolean {
  const apt = on.filter((track) => trackHas(track, field));
  return apt.length > 0 && apt.every((track) => trackFlag(track, field));
}

/**
 * Every track of the lane that `field` applies to ends with `value`, in one change; nothing for those already so (a
 * zoom track has no solo).
 */
export function setLaneOps(on: readonly Track[], field: TrackToggle, value: boolean): Op[] {
  return on.flatMap((track) => setTrackOps(track, field, value));
}
