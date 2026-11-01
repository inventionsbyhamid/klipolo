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
import { PositionField, TimingField } from './common';
import { CropField, isNoCrop } from './CropField';
import { Field } from './fields';
import { setClipSpeed } from './clipSpeed';
import { PictureFields } from './ImageInspector';
import { SpeedField } from './SpeedField';
import { cameraClipOf, mirrorLabel } from './videoOps';
import { VolumeControls } from './VolumeControls';

export type VideoContent = Extract<ItemContent, { type: 'video' }>;

/**
 * A video layer's fields (a picture in picture): Size, Opacity and Corner radius (`PictureFields`), Framing (Fit shows
 * the whole picture in its box, Fill covers the box from the centre, cutting the edges) and Crop (`CropField`), its
 * sound (Volume and Mute, when its file has sound), Speed (its length follows), Mirror, Position and Timing. A separated camera keeps
 * its clip's speed, so the lips stay in sync with the mic: its Speed changes the clip's as the clip card's does, saying
 * what that does to the clip's transitions. Each change is built at its turn from the layer as the project has it then
 * (`contentOps`), setting only its own field: Speed keeps a volume change still on its way.
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
  const framing = t({ id: 'clip.framing', message: 'Framing' });
  return (
    <>
      <PictureFields item={item} format={format} primary={primary} />
      <Field label={framing}>
        <Segmented
          label={framing}
          value={content.fill ? 'fill' : 'fit'}
          options={[
            {
              value: 'fit',
              label: t({ id: 'clip.framing.fit', message: 'Fit', comment: 'Show the whole video inside the frame' }),
              tooltip: t({ id: 'layer.framing.fit.tip', message: 'Show the whole video in its box' }),
            },
            {
              value: 'fill',
              label: t({ id: 'clip.framing.fill', message: 'Fill', comment: 'Fill the frame with the video' }),
              tooltip: t({ id: 'layer.framing.fill.tip', message: 'Fill its box, cropping the edges' }),
            },
          ]}
          fill
          onChange={(mode) =>
            void editContent(mode === 'fit' ? 'clip.framing.fit.undo' : 'clip.framing.fill.undo', item, content, {
              fill: mode === 'fill',
            })
          }
        />
      </Field>
      <CropField
        crop={content.crop}
        latest={() => contentNow(item.id, content).crop}
        ops={(crop) => contentOps(item, content, { crop: isNoCrop(crop) ? undefined : crop })}
      />
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
      <TimingField item={item} />
    </>
  );
}
