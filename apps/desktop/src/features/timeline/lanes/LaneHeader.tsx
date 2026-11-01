import { useLingui } from '@lingui/react/macro';
import { Eye, EyeOff, Headphones, Lock, Volume2, VolumeX, type LucideIcon } from 'lucide-react';
import { useId } from 'react';
import type { Track } from '../../../bindings/Track';
import { IconButton } from '../../../design/IconButton';
import { Menu, MenuContent, MenuItem, MenuSeparator, MenuTrigger } from '../../../design/Menu';
import { Icon, type IconName } from '../../../layout/Icon';
import { labelText } from '../../../lib/labels';
import { soloedTracks } from '../../../lib/solo';
import { useProject } from '../../../state/project';
import { setLaneFlag, soloOff, soloOffLabel } from '../keys/trackActions';
import { laneToggleLabel, trackStateName, type TrackState } from '../keys/trackLabels';
import { laneFlag, laneOff, laneToggles, laneTracks, type LaneId } from '../model/laneOps';

const icon = (Glyph: LucideIcon) => <Glyph size={14} strokeWidth={1.8} aria-hidden />;

/** What lane `lane`'s tracks `on` are at rest, if anything: hidden or muted, soloed, locked, or some of those. */
function restState(lane: LaneId, on: readonly Track[]): TrackState | null {
  const audio = lane !== 'layers';
  const words = [
    laneFlag(on, laneOff(lane)) ? (audio ? 'muted' : 'hidden') : '',
    laneFlag(on, 'solo') ? 'soloed' : '',
    laneFlag(on, 'locked') ? 'locked' : '',
  ].filter(Boolean);
  if (words.length === 0) return null;
  // "muted" + "Soloed" + "Locked": mutedSoloedLocked, one of `TrackState`'s names.
  return words.map((w, i) => (i === 0 ? w : w[0].toUpperCase() + w.slice(1))).join('') as TrackState;
}

interface LaneHeaderProps {
  lane: LaneId;
  /** The lane's name in the language shown ("Overlays"), which names its toolbar and its controls. */
  name: string;
  icon: IconName;
  /** Every track: the header acts on those the lane draws (`laneTracks`). */
  tracks: Track[];
}

/**
 * A lane's header, in the gutter: its icon, which opens the lane's options (Hide or Mute, Solo, Lock), and Hide or
 * Mute, in a toolbar named after the lane, as a recorded sound's lane has its volume and Mute. Each acts on every track
 * the lane draws, one undo step (`setLaneFlag`); its name says what it does, so it has no pressed state as well. At
 * rest a small badge on the icon shows a soloed or locked lane, and the options button says what the lane is.
 */
export function LaneHeader({ lane, name, icon: iconName, tracks }: LaneHeaderProps) {
  const { t } = useLingui();
  const stateId = useId();
  const on = laneTracks(tracks, lane);
  const off = laneOff(lane);
  const isOff = laneFlag(on, off);
  const soloed = laneFlag(on, 'solo');
  const locked = laneFlag(on, 'locked');
  const state = restState(lane, on);
  const offIcon = off === 'hidden' ? (isOff ? EyeOff : Eye) : isOff ? VolumeX : Volume2;
  return (
    <div role="toolbar" aria-label={name} className="tl-lane-head" data-lane-header={lane}>
      <Menu>
        <MenuTrigger asChild>
          <IconButton
            className="tl-lane-btn"
            icon={
              <>
                <Icon name={iconName} />
                {(soloed || locked) && (
                  <span className="tl-lane-badge" aria-hidden>
                    {soloed && <Headphones size={8} strokeWidth={2.4} />}
                    {locked && <Lock size={8} strokeWidth={2.4} />}
                  </span>
                )}
              </>
            }
            label={t({
              id: 'timeline.lane.options',
              message: `${name} options`,
              comment: "A timeline lane's menu button; {name} is the lane's name, e.g. Overlays",
            })}
            tooltip={name}
            aria-describedby={state ? stateId : undefined}
            data-lane-menu={lane}
          />
        </MenuTrigger>
        <MenuContent>
          <LaneMenuItems lane={lane} />
        </MenuContent>
      </Menu>
      {state && (
        <span id={stateId} hidden>
          {trackStateName(state)}
        </span>
      )}
      <IconButton
        className="tl-lane-btn"
        icon={icon(offIcon)}
        label={labelText(laneToggleLabel(lane, off, !isOff))}
        onClick={() => setLaneFlag(lane, off)}
      />
    </div>
  );
}

/**
 * The lane's options: its toggles, worded for what a press does, and Turn off solo while any track is soloed. Mounted
 * only while the menu is open, so only an open menu follows the tracks.
 */
function LaneMenuItems({ lane }: { lane: LaneId }) {
  const tracks = useProject((s) => s.snapshot?.project.timeline.tracks);
  if (!tracks) return null;
  const on = laneTracks(tracks, lane);
  const anySolo = soloedTracks(tracks).length > 0;
  return (
    <>
      {laneToggles(lane, on).map((field) => (
        <MenuItem key={field} onSelect={() => setLaneFlag(lane, field)}>
          {labelText(laneToggleLabel(lane, field, !laneFlag(on, field)))}
        </MenuItem>
      ))}
      {anySolo && (
        <>
          <MenuSeparator />
          <MenuItem onSelect={() => void soloOff()}>{soloOffLabel()}</MenuItem>
        </>
      )}
    </>
  );
}
