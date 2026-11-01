import { useLingui } from '@lingui/react/macro';
import type { Clip } from '../../bindings/Clip';
import type { FormatKind } from '../../bindings/FormatKind';
import { IconButton } from '../../design/IconButton';
import { Menu, MenuContent, MenuItem, MenuSeparator, MenuTrigger } from '../../design/Menu';
import { useIntlLocale } from '../../i18n/react';
import { Icon } from '../../layout/Icon';
import { formatNumber } from '../../lib/format';
import { clipOp } from '../../lib/ops';
import { clipHasSound, sourceLabel } from '../../lib/timeline';
import { clipNow } from '../../state/latest';
import { useProject } from '../../state/project';
import { useSelection } from '../../state/selection';
import { deleteBlock, duplicate } from '../timeline/commands';
import { ClipCameraFields } from './CameraFields';
import { ClipFadesField } from './ClipFadesField';
import { EffectsButton } from './EffectsButton';
import { setClipSpeed } from './clipSpeed';
import { cardEscape } from './common';
import { FramingField } from './FramingField';
import { LookField } from './LookField';
import { SourceMixTools } from './SourceMixTools';
import { SpeedField } from './SpeedField';
import { VolumeControls } from './VolumeControls';

/**
 * The clip card: the inspector for a selected main-track clip, in the left column like the others. Its header has the
 * film icon and the clip's name (its file's, or a recording's own: "Recording 1"), a more menu (Duplicate, Delete: the
 * timeline's own commands; a main clip has no layer order) and a close button. Its fields: Fit or Fill in the format
 * shown, and the Fill focus while Fill crops (`FramingField`); Speed (0.5x to 2x, the clip's length follows); when
 * the clip has sound, Volume (0 to 200%) and Mute; its look (`LookField`, which opens the Looks view); its effects
 * (`EffectsButton`); its fades from and to black (`ClipFadesField`); and for a recording clip its Sounds, each recorded
 * sound's own mute, volume and sync and the camera's sync (`SourceMixTools`), then its camera's layout, shape, border
 * and mirror (`ClipCameraFields`). Each change is one undo step.
 */
export function ClipInspector({ clip, format }: { clip: Clip; format: FormatKind }) {
  const { t } = useLingui();
  const locale = useIntlLocale();
  const project = useProject((s) => s.snapshot?.project);
  if (!project) return null;
  const title = sourceLabel(project, clip.source);
  const source = clip.source;
  const recording = source.type === 'recording' ? project.recordings.find((r) => r.id === source.id) : undefined;
  const close = () => useSelection.getState().select(null);
  const setSpeed = (speed: number) => setClipSpeed(clip, speed, title, `${formatNumber(locale, speed)}×`);
  return (
    <aside
      className="inspector"
      aria-label={t({ id: 'inspector.label', message: `${title} settings`, comment: '{title}: Title, Image, Music' })}
      onKeyDown={cardEscape(close)}
    >
      <div className="insp-head">
        <h3 title={title}>
          <Icon name="film" />
          <span className="insp-name">{title}</span>
        </h3>
        <Menu>
          <MenuTrigger asChild>
            <button type="button" className="icon-btn" aria-label={t({ id: 'inspector.more', message: 'More' })}>
              <Icon name="more" />
            </button>
          </MenuTrigger>
          <MenuContent align="end">
            <MenuItem onSelect={() => duplicate(clip.id)}>
              {t({ id: 'inspector.duplicate', message: 'Duplicate' })}
            </MenuItem>
            <MenuSeparator />
            <MenuItem onSelect={() => deleteBlock(clip.id, 'undo.delete.clip')}>
              {t({ id: 'inspector.delete', message: 'Delete' })}
            </MenuItem>
          </MenuContent>
        </Menu>
        <IconButton
          icon={<Icon name="x" />}
          label={t({ id: 'inspector.close', message: 'Close' })}
          shortcut="Esc"
          onClick={close}
        />
      </div>
      <div className="insp-body">
        <FramingField clip={clip} format={format} />
        <SpeedField speed={clip.speed} onPick={setSpeed} />
        {clipHasSound(project, clip) && (
          <VolumeControls
            volume={clip.volume}
            noun="clip"
            volumeOps={(volume) => [clipOp(clip, { volume })]}
            mute={{
              id: clip.id,
              muted: clip.muted,
              latest: () => clipNow(clip.id)?.muted ?? clip.muted,
              ops: (muted) => [clipOp(clip, { muted })],
            }}
          />
        )}
        <LookField clip={clip} />
        <EffectsButton target={{ type: 'clip', id: clip.id }} effects={clip.effects} />
        <ClipFadesField clip={clip} />
        {recording && <SourceMixTools clip={clip} recording={recording} />}
        <ClipCameraFields project={project} clip={clip} format={format} />
      </div>
    </aside>
  );
}
