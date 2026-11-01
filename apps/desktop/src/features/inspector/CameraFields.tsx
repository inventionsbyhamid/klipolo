import { useLingui } from '@lingui/react/macro';
import type { CameraLayout } from '../../bindings/CameraLayout';
import type { CameraMode } from '../../bindings/CameraMode';
import type { CameraShape } from '../../bindings/CameraShape';
import type { Clip } from '../../bindings/Clip';
import type { FormatKind } from '../../bindings/FormatKind';
import type { Project } from '../../bindings/Project';
import { Segmented } from '../../design/Segmented';
import { cameraModePatch, hasCameraOverlay, isCameraOnly, separatedCamera } from '../../lib/camera';
import type { Label } from '../../lib/labels';
import { cameraModeFor } from '../../lib/layout';
import { cameraOps, commitLatest, type PatchOf } from '../../state/latest';
import { useSelection } from '../../state/selection';
import { Field } from './fields';

/** Sets `patch` on clip `clip`'s camera, built at its turn on the camera as the project has it then: one undo step. */
const editCamera = (label: Label, clip: Clip, camera: CameraLayout, patch: PatchOf<CameraLayout>) =>
  void commitLatest(label, () => cameraOps(clip, camera, patch));

/** Mirror's row: Off or On, under the camera's own undo labels. */
export function CameraMirror({ clip, camera }: { clip: Clip; camera: CameraLayout }) {
  const { t } = useLingui();
  const mirror = t({ id: 'camera.mirror', message: 'Mirror' });
  return (
    <Field label={mirror}>
      <Segmented
        label={mirror}
        value={camera.mirror ? 'on' : 'off'}
        options={[
          { value: 'off', label: t({ id: 'media.mirror.off', message: 'Off', comment: 'Mirror off' }) },
          { value: 'on', label: t({ id: 'media.mirror.on', message: 'On', comment: 'Mirror on' }) },
        ]}
        fill
        onChange={(on) =>
          editCamera(on === 'on' ? 'camera.mirror.undo' : 'camera.unmirror.undo', clip, camera, {
            mirror: on === 'on',
          })
        }
      />
    </Field>
  );
}

/**
 * A recording's camera in the format shown: Bubble, Split (9:16 only) or Hidden, set in this format only; as a bubble
 * its shape and border; mirrored wherever it shows. `layoutLabel` names the first row ("Layout" on the Camera card,
 * "Camera" on the clip card). Each change is one undo step built at its turn from the camera as the project has it
 * then, setting only its own field: a drag of the bubble or another of these still on its way is kept.
 */
export function CameraFields({
  clip,
  camera,
  format,
  layoutLabel,
}: {
  clip: Clip;
  camera: CameraLayout;
  format: FormatKind;
  layoutLabel: string;
}) {
  const { t } = useLingui();
  const mode = cameraModeFor(camera, format);
  const layouts: { value: CameraMode; label: string; tooltip: string }[] = [
    {
      value: 'bubble',
      label: t({
        id: 'camera.layout.bubble',
        message: 'Bubble',
        comment: 'Camera layout: a small camera over the video',
      }),
      tooltip: t({ id: 'camera.layout.bubble.tip', message: 'A small camera over the video' }),
    },
    ...(format === 'portrait' || mode === 'splitBottom'
      ? [
          {
            value: 'splitBottom' as const,
            label: t({ id: 'camera.layout.split', message: 'Split', comment: 'Camera layout: below the screen' }),
            tooltip: t({ id: 'camera.layout.split.tip', message: 'The camera below the screen' }),
          },
        ]
      : []),
    {
      value: 'hidden',
      label: t({ id: 'camera.layout.hidden', message: 'Hidden', comment: 'Camera layout: no camera' }),
      tooltip: t({ id: 'camera.layout.hidden.tip', message: 'No camera in this format' }),
    },
  ];
  const shapes: { value: CameraShape; label: string }[] = [
    { value: 'circle', label: t({ id: 'camera.shape.circle', message: 'Circle' }) },
    { value: 'rounded', label: t({ id: 'camera.shape.rounded', message: 'Rounded' }) },
    { value: 'square', label: t({ id: 'camera.shape.square', message: 'Square' }) },
  ];
  const shape = t({ id: 'camera.shape.row', message: 'Shape' });
  const border = t({ id: 'camera.border', message: 'Border' });
  return (
    <>
      <Field label={layoutLabel}>
        <Segmented
          label={t({ id: 'camera.layout', message: 'Camera layout' })}
          value={mode}
          options={layouts}
          fill
          onChange={(next) =>
            editCamera('camera.layout.undo', clip, camera, (now) => cameraModePatch(now, next, format))
          }
        />
      </Field>
      {mode === 'bubble' && (
        <>
          <Field label={shape}>
            <Segmented
              label={t({ id: 'camera.shape', message: 'Camera shape' })}
              value={camera.shape}
              options={shapes}
              fill
              onChange={(next) => editCamera('camera.shape.undo', clip, camera, { shape: next })}
            />
          </Field>
          <Field label={border}>
            <Segmented
              label={border}
              value={camera.border ? 'on' : 'off'}
              options={[
                { value: 'off', label: t({ id: 'camera.border.off', message: 'Off', comment: 'Border off' }) },
                { value: 'on', label: t({ id: 'camera.border.on', message: 'On', comment: 'Border on' }) },
              ]}
              fill
              onChange={(on) =>
                editCamera(on === 'on' ? 'camera.border.show.undo' : 'camera.border.hide.undo', clip, camera, {
                  border: on === 'on',
                })
              }
            />
          </Field>
        </>
      )}
      {mode !== 'hidden' && <CameraMirror clip={clip} camera={camera} />}
    </>
  );
}

/**
 * A recording clip's camera on its card: the camera's controls while it shows beside the screen (the way back to a
 * camera hidden in the format shown), the way to its layer once it is a layer of its own (its controls are that
 * layer's then), and Mirror for a take of the camera alone (the camera is the clip's footage). Nothing for any other
 * clip.
 */
export function ClipCameraFields({ project, clip, format }: { project: Project; clip: Clip; format: FormatKind }) {
  const { t } = useLingui();
  const camera = clip.camera;
  if (!camera) return null;
  if (isCameraOnly(project, clip)) return <CameraMirror clip={clip} camera={camera} />;
  if (!hasCameraOverlay(project, clip)) return null;
  const label = t({ id: 'layer.camera', message: 'Camera', comment: 'A recording’s camera picture' });
  const layer = separatedCamera(project, clip);
  if (layer)
    return (
      <Field label={label}>
        <div className="insp-note">
          <p className="field-note">
            {t({ id: 'clip.camera.layer.note', message: 'The camera is its own layer now.' })}
          </p>
          <button
            type="button"
            className="btn btn-secondary btn-sm"
            onClick={() => useSelection.getState().select(layer.id)}
          >
            {t({ id: 'clip.camera.layer', message: 'Select camera layer' })}
          </button>
        </div>
      </Field>
    );
  return <CameraFields clip={clip} camera={camera} format={format} layoutLabel={label} />;
}
