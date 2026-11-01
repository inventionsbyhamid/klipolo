import { useLingui } from '@lingui/react/macro';
import type { CameraLayout } from '../../bindings/CameraLayout';
import type { Clip } from '../../bindings/Clip';
import type { FormatKind } from '../../bindings/FormatKind';
import { IconButton } from '../../design/IconButton';
import { Icon } from '../../layout/Icon';
import { useSelection } from '../../state/selection';
import { CameraFields } from './CameraFields';
import { cardEscape } from './common';

/**
 * The Camera card, in the left column: a recording's camera, selected on the video. Its header has the camera
 * icon, its name and a close button (it is part of its clip, so it has no layer actions); its fields the camera's
 * layout in the format shown, its shape and border as a bubble, and its mirror (`CameraFields`).
 */
export function CameraInspector({ clip, camera, format }: { clip: Clip; camera: CameraLayout; format: FormatKind }) {
  const { t } = useLingui();
  const title = t({ id: 'layer.camera', message: 'Camera', comment: 'A recording’s camera picture' });
  const close = () => useSelection.getState().select(null);
  return (
    <aside
      className="inspector"
      aria-label={t({ id: 'inspector.label', message: `${title} settings`, comment: '{title}: Title, Image, Music' })}
      onKeyDown={cardEscape(close)}
    >
      <div className="insp-head">
        <h3>
          <Icon name="camera" />
          {title}
        </h3>
        <IconButton
          icon={<Icon name="x" />}
          label={t({ id: 'inspector.close', message: 'Close' })}
          shortcut="Esc"
          onClick={close}
        />
      </div>
      <div className="insp-body">
        <CameraFields
          clip={clip}
          camera={camera}
          format={format}
          layoutLabel={t({ id: 'camera.layout.row', message: 'Layout' })}
        />
      </div>
    </aside>
  );
}
