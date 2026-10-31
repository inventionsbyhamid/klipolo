import { t } from '@lingui/core/macro';
import { useCallback } from 'react';
import { newTextItem } from '../../lib/defaults';
import { mainDurationUs, showItemAtUs, videoDurationUs } from '../../lib/timeline';
import { usePlayback } from '../../state/playback';
import { useProject } from '../../state/project';
import { useTextEdit } from './textEdit';

const TEXT_US = 3_000_000;

/**
 * Opens a new 3-second title at the playhead on the canvas, ready to type with "Your text" selected, where Rust's Title
 * preset puts one in each format (`newTextItem`). It is a draft until the edit finishes: with words it becomes one "Add text" step, empty it leaves nothing. Inside the
 * video it will be attached, so it moves with the clip under it when earlier parts are cut. The playhead moves past
 * the title's fade-in, so the paused preview shows it once added (at its start it is fully transparent).
 */
export function useAddText(): () => Promise<void> {
  return useCallback(async () => {
    const snapshot = useProject.getState().snapshot;
    if (!snapshot) return;
    // Typing pauses: the box goes where the picture stopped.
    if (usePlayback.getState().playing) usePlayback.getState().pause();
    const playheadUs = usePlayback.getState().playheadUs;
    // Placed in every format at once: in 9:16 inside what the apps leave clear, whichever format is shown.
    const item = newTextItem(
      t({ id: 'text.placeholder', message: 'Your text', comment: 'The words a new title starts with' }),
      playheadUs,
      TEXT_US,
      snapshot.project.settings.primaryFormat,
    );
    // It follows the clip under it, where there is one; past the main track's end only layers play.
    useTextEdit.getState().beginDraft(item, playheadUs < mainDurationUs(snapshot.project.timeline.main));
    const videoUs = videoDurationUs(snapshot.project.timeline);
    // Never past the end of the video, like every seek.
    const shown = showItemAtUs(item, snapshot.project.settings.fps);
    usePlayback.getState().seek(Math.max(playheadUs, Math.min(shown, videoUs)));
  }, []);
}
