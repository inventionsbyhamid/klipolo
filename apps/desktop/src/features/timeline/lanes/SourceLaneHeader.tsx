import { useLingui } from '@lingui/react/macro';
import {
  AppWindow,
  Gamepad2,
  Headphones,
  Mic,
  MonitorSpeaker,
  Video,
  Volume2,
  VolumeX,
  type LucideIcon,
} from 'lucide-react';
import { memo } from 'react';
import type { ErrorParam } from '../../../bindings/ErrorParam';
import type { RecordingSource } from '../../../bindings/RecordingSource';
import type { SourceRole } from '../../../bindings/SourceRole';
import { IconButton } from '../../../design/IconButton';
import { Popover, PopoverContent, PopoverTrigger } from '../../../design/Popover';
import { labelText } from '../../../lib/labels';
import { isVideoRole } from '../../../lib/recording';
import { ToolSlider } from '../../inspector/ToolSlider';
import { kindVolumeOpsNow, setKindMuted, soundUndoLabel } from '../keys/sourceActions';

const percent = (v: number) => `${Math.round(v * 100)}%`;

/** Each kind of sound's icon, which opens its volume: a picture's own sound has the video icon. */
const ICONS: Partial<Record<SourceRole, LucideIcon>> = {
  mic: Mic,
  systemAudio: MonitorSpeaker,
  gameAudio: Gamepad2,
  voiceChat: Headphones,
  otherApp: AppWindow,
};

const icon = (Icon: LucideIcon) => <Icon size={14} strokeWidth={1.8} aria-hidden />;

interface SourceLaneHeaderProps {
  role: SourceRole;
  /** Which of a recording's sources of `role` the lane is (0 the first: "Mic", 1: "Mic 2"). */
  nth: number;
  /** Every played recording's source of this kind. */
  sources: readonly RecordingSource[];
  /** Its name in the language shown ("Mic", "Camera sound"). */
  name: string;
  /** The same as an undo label's value, worded wherever the label is read. */
  param: ErrorParam;
  /** Some of its sound is on layers of its own, which keep their own volume and mute. */
  separated: boolean;
}

/**
 * A kind of recorded sound's lane header, in the gutter: its icon, which opens its volume (a slider in a popover,
 * committed on release), and Mute, in a toolbar named after the lane, so lanes with the same icon ("Mic", "Mic 2") are
 * told apart. Both act on that sound alone (each recording's `nth` source of `role`) in every recording and every
 * clip (`setKindMuted`, `kindVolumeOpsNow`), each one undo step. Muted shows when every recording's is; the volume
 * shown is the first's. The Mute button's name says what it does, so it has no pressed state as well.
 */
function SourceLaneHeaderView({ role, nth, sources, name, param, separated }: SourceLaneHeaderProps) {
  const { t } = useLingui();
  const first = sources[0];
  const muted = sources.every((s) => s.mix.muted);
  const mute = labelText(soundUndoLabel(muted ? 'undo.source.unmute' : 'undo.source.mute', param));
  const volumeName = t({
    id: 'source.volume',
    message: `${name} volume`,
    comment: "A recorded sound's volume; {name} is e.g. Mic or Computer sound",
  });
  return (
    <div role="toolbar" aria-label={name} className="tl-source-head" data-source-header>
      <Popover>
        <PopoverTrigger asChild>
          <IconButton
            className="tl-source-btn"
            icon={icon(isVideoRole(role) ? Video : (ICONS[role] ?? Mic))}
            label={volumeName}
          />
        </PopoverTrigger>
        <PopoverContent aria-label={volumeName} side="right" align="start" className="source-volume">
          <ToolSlider
            label={t({ id: 'volume.label', message: 'Volume' })}
            commitLabel={soundUndoLabel('undo.source.volume', param)}
            value={first.mix.volume}
            min={0}
            max={2}
            step={0.01}
            format={percent}
            ops={(v) => kindVolumeOpsNow(role, v, nth)}
            preview={false}
          />
          <p className="field-note">
            {separated
              ? t({
                  id: 'source.volume.noteSeparated',
                  message:
                    'For every clip, also those with their own setting. Its separated sound keeps its own volume.',
                  comment: "Under a recorded sound's volume slider; part of the sound is on a layer of its own",
                })
              : t({
                  id: 'source.volume.note',
                  message: 'For every clip, also those with their own setting.',
                  comment: "Under a recorded sound's volume slider: it sets the sound in every clip of every recording",
                })}
          </p>
        </PopoverContent>
      </Popover>
      <IconButton
        className="tl-source-btn"
        icon={icon(muted ? VolumeX : Volume2)}
        label={mute}
        onClick={() => setKindMuted(role, !muted, nth, param)}
      />
    </div>
  );
}

/** The same header: its sources are rebuilt on each draw of the lanes, so they are compared one by one. */
function sameHeader(a: SourceLaneHeaderProps, b: SourceLaneHeaderProps): boolean {
  return (
    a.role === b.role &&
    a.nth === b.nth &&
    a.name === b.name &&
    a.separated === b.separated &&
    a.sources.length === b.sources.length &&
    a.sources.every((source, i) => source === b.sources[i]) &&
    JSON.stringify(a.param) === JSON.stringify(b.param)
  );
}

/** Drawn again only when its sources change, not for a zoom, a scroll or a selection. */
export const SourceLaneHeader = memo(SourceLaneHeaderView, sameHeader);
