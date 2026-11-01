import { t } from '@lingui/core/macro';
import type { Clip } from '../../bindings/Clip';
import { announce } from '../../state/announce';
import { editClip } from '../../state/latest';
import { useProject } from '../../state/project';
import { speedNote } from '../timeline/commands';

/**
 * Main clip `clip` at `speed`, in one step. The model refits the transitions at its cuts, and the change says when it
 * shortens or removes one, in its undo label and aloud. `title` names the clip and `shown` the speed (e.g. 2×).
 */
export function setClipSpeed(clip: Clip, speed: number, title: string, shown: string) {
  const now = useProject.getState().snapshot?.project;
  const note = now ? speedNote(now, clip.id, speed) : '';
  const said = note
    ? t({
        id: 'clip.speed.saidNote',
        message: `Changed the speed of ${title} to ${shown}. ${note}`,
        comment: 'Screen-reader note; {shown} is e.g. 2×; {note} says what it did to transitions',
      })
    : t({
        id: 'clip.speed.said',
        message: `Changed the speed of ${title} to ${shown}`,
        comment: 'Screen-reader note; {shown} is e.g. 2×',
      });
  void editClip(note ? { id: 'speed.undo.note', params: { note } } : 'speed.undo', clip, { speed }).then(
    (ok) => ok && announce(said),
  );
}
