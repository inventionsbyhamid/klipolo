import { useLingui } from '@lingui/react/macro';
import { Volume2, VolumeX } from 'lucide-react';
import type { Op } from '../../bindings/Op';
import { IconButton } from '../../design/IconButton';
import { commitToggle } from '../../state/commit';
import { muteLabel } from '../preview';
import { Field } from './fields';
import { ToolSlider } from './ToolSlider';

const percent = (v: number) => `${Math.round(v * 100)}%`;

interface VolumeControlsProps {
  volume: number;
  volumeOps: (volume: number) => Op[];
  /**
   * Which layer it mutes (`id`), whether it is muted (as drawn, and `latest`: as the project has it when a press goes
   * in), and the ops that leave it muted or not (its own switch, so its volume stays for when it plays again). A press
   * is decided and built as it goes in, after a change still on its way (`commitToggle`).
   */
  mute?: {
    id: string;
    muted: boolean;
    latest: () => boolean;
    ops: (muted: boolean) => Op[];
  };
  /** What the layer is called in the undo labels: "video" makes "Mute video layer" (`volume.mute.undo`). */
  noun: 'clip' | 'video' | 'sound';
}

/**
 * The Volume row: its slider (0 to 200 %, committed on release; sound isn't dry-run) and, when it has one, a Mute
 * toggle beside it.
 */
export function VolumeControls({ volume, volumeOps, mute: muting, noun }: VolumeControlsProps) {
  const { t } = useLingui();
  const label = t({ id: 'volume.label', message: 'Volume' });
  const mute = t({ id: 'volume.mute', message: 'Mute' });
  const muted = muting?.muted ?? false;
  return (
    <Field label={label}>
      <div className="volume-row">
        <ToolSlider
          label={label}
          commitLabel="volume.undo"
          value={volume}
          min={0}
          max={2}
          step={0.01}
          format={percent}
          preview={false}
          ops={volumeOps}
        />
        {muting && (
          <IconButton
            className="icon-btn-sm"
            icon={muted ? VolumeX : Volume2}
            label={mute}
            tooltip={muted ? t({ id: 'volume.unmute', message: 'Unmute' }) : mute}
            active={muted}
            onClick={() =>
              void commitToggle(muting.latest, (on) => muteLabel(on, noun), muting.ops, { id: muting.id })
            }
          />
        )}
      </div>
    </Field>
  );
}
