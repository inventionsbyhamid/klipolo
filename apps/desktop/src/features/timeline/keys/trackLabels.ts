import { select, t } from '@lingui/core/macro';
import type { Track } from '../../../bindings/Track';
import { labelText, type LabelId } from '../../../lib/labels';
import type { TrackToggle } from '../model/trackOps';

/* What a track's toggles say: in its header (naming the track), and in its menu, the palette and the undo history. */

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
