import { useRecorder } from '../../state/recorder';
import { useRecorderPreviews } from '../../state/recorderPreviews';
import { LivePicture, useStale } from './LivePreview';

/**
 * The camera bubble: a 96 px circle of the chosen camera over the preview's bottom right, 12 px in,
 * mirrored while "Mirror my camera" is on (the picture only: the camera's own frames never are). Nothing until its
 * first frame; grey once its frames stop coming.
 */
export function CameraBubble() {
  const url = useRecorderPreviews((s) => s.camera);
  const at = useRecorderPreviews((s) => s.cameraAt);
  const mirror = useRecorder((s) => s.picks.mirror);
  const stale = useStale(at);
  if (url === null) return null;
  return (
    <div
      className="record-bubble"
      role="img"
      aria-label="Your camera"
      data-mirror={mirror || undefined}
      data-stale={stale || undefined}
    >
      <LivePicture url={url} />
    </div>
  );
}

/**
 * "Record yourself"'s picture: the chosen camera over the whole preview box, fitted over black, mirrored while "Mirror
 * my camera" is on (the picture only: the recorded file never is). Nothing until its first frame; grey once its frames
 * stop coming.
 */
export function CameraView() {
  const url = useRecorderPreviews((s) => s.camera);
  const at = useRecorderPreviews((s) => s.cameraAt);
  const mirror = useRecorder((s) => s.picks.mirror);
  const stale = useStale(at);
  if (url === null) return null;
  return (
    <div
      className="record-camera"
      role="img"
      aria-label="Your camera"
      data-mirror={mirror || undefined}
      data-stale={stale || undefined}
    >
      <LivePicture url={url} />
    </div>
  );
}
