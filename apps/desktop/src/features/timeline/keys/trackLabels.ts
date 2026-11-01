import { select, t } from '@lingui/core/macro';
import type { Track } from '../../../bindings/Track';
import { labelText, type Label, type LabelId } from '../../../lib/labels';
import type { LaneId } from '../model/laneOps';
import type { TrackToggle } from '../model/trackOps';

/*
 * What a track's toggles say: in its header (naming the track), and in its menu, the palette and the undo history; and
 * a lane's, naming the lane.
 */

/**
 * The undo label of track `track`'s toggle for `field`, which is also what the toggle says it does: "Hide track",
 * "Unmute track" (null: no track, as one that's shown, unmuted and unlocked).
 */
export function trackToggleId(track: Pick<Track, TrackToggle> | null, field: TrackToggle): LabelId {
  if (field === 'solo') return track?.solo ? 'timeline.track.unsolo' : 'timeline.track.solo';
  if (field === 'hidden') return track?.hidden ? 'timeline.track.show' : 'timeline.track.hide';
  if (field === 'muted') return track?.muted ? 'timeline.track.unmute' : 'timeline.track.mute';
  return track?.locked ? 'timeline.track.unlock' : 'timeline.track.lock';
}

/** What the toggle says it does, in the language shown: "Hide track", "Unmute track". */
export const trackToggleLabel = (track: Pick<Track, TrackToggle> | null, field: TrackToggle): string =>
  labelText(trackToggleId(track, field));

/**
 * What track `track`'s button for `field` does, naming the track: "Hide Overlay 1", "Unmute Music". The header's
 * buttons say it, so a screen reader tells one row's from another's.
 */
export function trackToggleName(track: Track, field: TrackToggle): string {
  const name = track.name;
  const action =
    field === 'hidden'
      ? track.hidden
        ? 'show'
        : 'hide'
      : field === 'muted'
        ? track.muted
          ? 'unmute'
          : 'mute'
        : field === 'solo'
          ? track.solo
            ? 'unsolo'
            : 'solo'
          : track.locked
            ? 'unlock'
            : 'lock';
  return t({
    id: 'timeline.track.toggleNamed',
    message: select(action, {
      hide: `Hide ${name}`,
      show: `Show ${name}`,
      mute: `Mute ${name}`,
      unmute: `Unmute ${name}`,
      solo: `Solo ${name}`,
      unsolo: `Unsolo ${name}`,
      lock: `Lock ${name}`,
      other: `Unlock ${name}`,
    }),
    comment: 'A track header’s button; {name} is the track’s name',
  });
}

/** What a track is at rest, as its header's badge shows it: hidden or muted, soloed, locked, or some of those. */
export type TrackState =
  | 'hidden'
  | 'muted'
  | 'soloed'
  | 'locked'
  | 'hiddenSoloed'
  | 'mutedSoloed'
  | 'hiddenLocked'
  | 'mutedLocked'
  | 'soloedLocked'
  | 'hiddenSoloedLocked'
  | 'mutedSoloedLocked';

/** The header badge's name: "Hidden", "Muted and locked", "Soloed and locked". */
export function trackStateName(state: TrackState): string {
  return t({
    id: 'timeline.track.state',
    message: select(state, {
      hidden: 'Hidden',
      muted: 'Muted',
      soloed: 'Soloed',
      locked: 'Locked',
      hiddenSoloed: 'Hidden and soloed',
      mutedSoloed: 'Muted and soloed',
      hiddenLocked: 'Hidden and locked',
      mutedLocked: 'Muted and locked',
      soloedLocked: 'Soloed and locked',
      hiddenSoloedLocked: 'Hidden, soloed and locked',
      other: 'Muted, soloed and locked',
    }),
    comment:
      'A track header’s badge for what the track is (hidden from the video, muted, soloed: only soloed tracks play, locked against edits)',
  });
}

/** Each lane's name as a label's value (`{lane}`), worded wherever the label is read. */
const LANE_NAMES: Record<LaneId, LabelId> = {
  layers: 'timeline.lane.overlays',
  sounds: 'timeline.lane.sounds',
  music: 'timeline.lane.music',
};

/** Lane `lane`'s name in the language shown: "Overlays", "Sound effects", "Music". */
export const laneName = (lane: LaneId): string => labelText(LANE_NAMES[lane]);

/**
 * The undo label of lane `lane`'s toggle for `field` that ends in `value`, which is also what the toggle says it does:
 * "Hide Overlays" (`value` true), "Unmute Music" (false).
 */
export function laneToggleLabel(lane: LaneId, field: TrackToggle, value: boolean): Label {
  const ids: Record<TrackToggle, [LabelId, LabelId]> = {
    hidden: ['timeline.lane.hide', 'timeline.lane.show'],
    muted: ['timeline.lane.mute', 'timeline.lane.unmute'],
    solo: ['timeline.lane.solo', 'timeline.lane.unsolo'],
    locked: ['timeline.lane.lock', 'timeline.lane.unlock'],
  };
  return { id: ids[field][value ? 0 : 1], params: { lane: { code: LANE_NAMES[lane], params: {} } } };
}

/**
 * What a track or lane named `name` is now that its `field` is `value`, for the live region: "Music is muted",
 * "Overlay 1 is shown".
 */
export function toggledSaid(name: string, field: TrackToggle, value: boolean): string {
  const state =
    field === 'hidden'
      ? value
        ? 'hidden'
        : 'shown'
      : field === 'muted'
        ? value
          ? 'muted'
          : 'unmuted'
        : field === 'solo'
          ? value
            ? 'soloed'
            : 'unsoloed'
          : value
            ? 'locked'
            : 'unlocked';
  return t({
    id: 'timeline.track.toggled',
    message: select(state, {
      hidden: `${name} is hidden`,
      shown: `${name} is shown`,
      muted: `${name} is muted`,
      unmuted: `${name} is unmuted`,
      soloed: `${name} is soloed: only soloed tracks play`,
      unsoloed: `${name} is no longer soloed`,
      locked: `${name} is locked`,
      other: `${name} is unlocked`,
    }),
    comment: 'Screen-reader note after a track’s toggle; {name} is the track’s name',
  });
}
