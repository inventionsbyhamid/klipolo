import { Trans } from '@lingui/react/macro';
import type { TrackKind } from '../../../bindings/TrackKind';
import { useDragState } from '../../../state/dragState';
import { NEW_TRACK_PX, TRACK_HEADER_PX } from '../layout';

/**
 * Where a dragged layer of `kind` can go to get a track of its own (every track showing): a dashed row named "New
 * track". `drag/dragView`'s `rowAt` finds it by `data-new-track`, and the drop adds the track and moves the layer onto
 * it in one undo step.
 */
export function NewTrackZone({ kind }: { kind: TrackKind }) {
  return (
    <div data-new-track={kind} className="tl-new-track" style={{ height: NEW_TRACK_PX }}>
      <div className="tl-gutter tl-new-track-name" style={{ width: TRACK_HEADER_PX }}>
        <Trans id="timeline.newTrack" comment="Where a dragged layer can be dropped to get a track of its own">
          New track
        </Trans>
      </div>
      <div className="tl-new-track-zone" />
    </div>
  );
}

/**
 * The new-track zone's place: above the picture tracks (an overlay or zoom layer's) or below the audio tracks (a
 * sound's), shown while a layer of that kind is dragged. Above the tracks its room is always kept, so a drag that opens
 * it moves no row out from under the pointer (a layer pressed near the top track's top edge would land on it).
 */
export function NewTrackSlot({ at }: { at: 'top' | 'bottom' }) {
  const kind = useDragState((s) => s.itemKind);
  if (kind !== null && (kind === 'audio') === (at === 'bottom')) return <NewTrackZone kind={kind} />;
  if (at === 'bottom') return null;
  return (
    <div aria-hidden className="tl-new-track" style={{ height: NEW_TRACK_PX }}>
      <div className="tl-gutter" style={{ width: TRACK_HEADER_PX }} />
    </div>
  );
}
