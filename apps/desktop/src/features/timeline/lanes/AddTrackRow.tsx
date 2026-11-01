import { useLingui } from '@lingui/react/macro';
import { Button } from '../../../design/Button';
import { Icon } from '../../../layout/Icon';
import { addTrack } from '../keys/trackActions';

/**
 * Under the audio tracks while every track shows: "Add overlay track" and "Add audio track", each one undo step. It
 * stays at the left while the lanes scroll.
 */
export function AddTrackRow() {
  const { t } = useLingui();
  return (
    <div data-add-track className="tl-add-track">
      <div className="tl-add-track-buttons">
        <Button variant="ghost" size="sm" onClick={() => addTrack('overlay')}>
          <Icon name="plus" />
          {t({ id: 'timeline.track.addOverlay', message: 'Add overlay track', comment: 'Button' })}
        </Button>
        <Button variant="ghost" size="sm" onClick={() => addTrack('audio')}>
          <Icon name="plus" />
          {t({ id: 'timeline.track.addAudio', message: 'Add audio track', comment: 'Button' })}
        </Button>
      </div>
    </div>
  );
}
