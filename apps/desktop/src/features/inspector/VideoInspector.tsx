import { useLingui } from '@lingui/react/macro';
import type { FormatKind } from '../../bindings/FormatKind';
import type { Item } from '../../bindings/Item';
import type { ItemContent } from '../../bindings/ItemContent';
import { Segmented } from '../../design/Segmented';
import { useIntlLocale } from '../../i18n/react';
import { formatNumber } from '../../lib/format';
import { hasOwnSound, sourceLabel } from '../../lib/timeline';
import { contentNow, contentOps, editContent } from '../../state/latest';
import { useProject } from '../../state/project';
import { AnimationField, PositionField, TimingField } from './common';
import { EffectsButton } from './EffectsButton';
import { Field } from './fields';
import { setClipSpeed } from './clipSpeed';
import { PictureFields } from './ImageInspector';
import { SpeedField } from './SpeedField';
import { cameraClipOf, mirrorLabel } from './videoOps';
import { VolumeControls } from './VolumeControls';

export type VideoContent = Extract<ItemContent, { type: 'video' }>;

/**
 * A video layer's fields (a picture in picture): Size, Opacity and Corner radius (`PictureFields`), Effects, its
 * sound (Volume and Mute, when its file has sound), Speed (its length follows), Mirror, Position, Animation and Timing.
 * A separated camera keeps its clip's speed, so the lips stay in sync with the mic: its Speed changes the clip's as the
 * clip card's does, saying what that does to the clip's transitions. Each change is built at its turn from the layer
 * as the project has it then (`contentOps`), setting only its own field: Speed keeps a volume change still on its way.
 */
export function VideoInspector({
  item,
  content,
  format,
  primary,
}: {
  item: Item;
  content: VideoContent;
  format: FormatKind;
  primary: FormatKind;
}) {
  const { t } = useLingui();
  const locale = useIntlLocale();
  const hasAudio = useProject((s) => !!s.snapshot && hasOwnSound(s.snapshot.project, item));
  const cameraClip = useProject((s) => (s.snapshot ? cameraClipOf(s.snapshot.project, item) : null));
  const cameraTitle = useProject((s) =>
    s.snapshot && cameraClip ? sourceLabel(s.snapshot.project, cameraClip.source) : '',
  );
  const speed = cameraClip ? cameraClip.speed : content.speed;
  const setSpeed = (value: number) =>
    void (cameraClip
      ? setClipSpeed(cameraClip, value, cameraTitle, `${formatNumber(locale, value)}×`)
      : editContent('speed.undo', item, content, { speed: value }));
  const mirror = t({ id: 'camera.mirror', message: 'Mirror' });
  return (
    <>
      <PictureFields item={item} format={format} primary={primary} />
      <EffectsButton target={{ type: 'item', id: item.id }} effects={item.effects} />
      {hasAudio && (
        <VolumeControls
          volume={content.volume}
          noun="video"
          volumeOps={(volume) => contentOps(item, content, { volume })}
          mute={{
            id: item.id,
            muted: content.muted,
            latest: () => contentNow(item.id, content).muted,
            ops: (muted) => contentOps(item, content, { muted }),
          }}
        />
      )}
      <SpeedField speed={speed} onPick={setSpeed} />
      {cameraClip && (
        <p className="field-note">
          {t({
            id: 'speed.camera.note',
            message: "Changes the clip's speed too, so the camera stays in sync.",
          })}
        </p>
      )}
      <Field label={mirror}>
        <Segmented
          label={mirror}
          value={content.mirror ? 'on' : 'off'}
          options={[
            { value: 'off', label: t({ id: 'media.mirror.off', message: 'Off', comment: 'Mirror off' }) },
            { value: 'on', label: t({ id: 'media.mirror.on', message: 'On', comment: 'Mirror on' }) },
          ]}
          fill
          onChange={(on) =>
            void editContent(mirrorLabel(cameraClip !== null, on === 'on'), item, content, { mirror: on === 'on' })
          }
        />
      </Field>
      <PositionField item={item} format={format} primary={primary} />
      <AnimationField item={item} typewriter={false} />
      <TimingField item={item} />
    </>
  );
}
