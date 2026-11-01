import { useLingui } from '@lingui/react/macro';
import { useId, useRef, type Ref } from 'react';
import type { Track } from '../../../bindings/Track';
import { IconButton } from '../../../design/IconButton';
import { Menu, MenuContent, MenuItem, MenuNote, MenuSeparator, MenuTrigger } from '../../../design/Menu';
import { Icon } from '../../../layout/Icon';
import { labelText } from '../../../lib/labels';
import { useProject } from '../../../state/project';
import { deleteTrack, moveTrack, setTrackFlag, startRename } from '../keys/trackActions';
import { trackToggleLabel } from '../keys/trackLabels';
import { trackStepIndex, trackToggles } from '../model/trackOps';

/** The track's options button: 24 px, so the name keeps the rest of the header's column (`TrackHeader`). */
export const TRACK_CONTROL_PX = 24;
const CONTROL_SIZE = { width: TRACK_CONTROL_PX, height: TRACK_CONTROL_PX };

interface TrackMenuProps {
  track: Track;
  /** The ids that describe the button: the track's name, and what it is ("Track options, Overlay 1, Locked"). */
  describedBy: string;
  ref?: Ref<HTMLButtonElement>;
}

/**
 * A track header's "Track options" menu: Rename; Hide (or Mute), Solo (not a zoom track's) and Lock; Move up and Move
 * down; Delete track. Each item runs the same action as the header's controls (`trackActions`).
 */
export function TrackMenu({ track, describedBy, ref }: TrackMenuProps) {
  const { t } = useLingui();
  // Rename starts once the menu has closed, so focus goes to the name field rather than back to this button.
  const renameNext = useRef(false);
  return (
    <Menu>
      <MenuTrigger asChild>
        <IconButton
          ref={ref}
          icon={<Icon name="more" />}
          label={t({ id: 'timeline.track.options', message: 'Track options' })}
          aria-describedby={describedBy}
          data-track-menu={track.id}
          // Its size from the constant the header's room is worked out with, not a class.
          style={CONTROL_SIZE}
        />
      </MenuTrigger>
      <MenuContent
        align="end"
        onCloseAutoFocus={(e) => {
          if (!renameNext.current) return;
          renameNext.current = false;
          e.preventDefault();
          startRename(track.id);
        }}
      >
        <TrackMenuItems track={track} onRename={() => (renameNext.current = true)} />
      </MenuContent>
    </Menu>
  );
}

/** The menu's items: mounted only while it is open, so only an open menu follows the tracks around this one. */
function TrackMenuItems({ track, onRename }: { track: Track; onRename: () => void }) {
  const { t } = useLingui();
  const noteId = useId();
  const tracks = useProject((s) => s.snapshot?.project.timeline.tracks);
  const up = tracks ? trackStepIndex(tracks, track.id, 1) !== null : false;
  const down = tracks ? trackStepIndex(tracks, track.id, -1) !== null : false;
  return (
    <>
      <MenuItem onSelect={onRename}>
        {t({ id: 'timeline.track.menu.rename', message: 'Rename', comment: "A track's menu item" })}
      </MenuItem>
      {/* The header's toggles, which show there at rest only while they are on. */}
      {trackToggles(track).map((field) => (
        // The other state from what the track has when the press goes in.
        <MenuItem key={field} onSelect={() => setTrackFlag(track.id, field)}>
          {trackToggleLabel(track, field)}
        </MenuItem>
      ))}
      <MenuSeparator />
      <MenuItem disabled={!up} onSelect={() => moveTrack(track.id, 1)}>
        {t({ id: 'timeline.track.menu.up', message: 'Move up', comment: "A track's menu item: one row up" })}
      </MenuItem>
      <MenuItem disabled={!down} onSelect={() => moveTrack(track.id, -1)}>
        {t({ id: 'timeline.track.menu.down', message: 'Move down', comment: "A track's menu item: one row down" })}
      </MenuItem>
      <MenuSeparator />
      <MenuItem
        disabled={track.locked}
        describedBy={track.locked ? noteId : undefined}
        onSelect={() => deleteTrack(track.id)}
      >
        {labelText('timeline.track.delete')}
      </MenuItem>
      {track.locked && (
        <MenuNote id={noteId}>
          {t({ id: 'timeline.track.lockedNote', message: 'Unlock the track to delete it.' })}
        </MenuNote>
      )}
    </>
  );
}
