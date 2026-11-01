import { t } from '@lingui/core/macro';
import { isCode } from '../../../bridge/call';
import { reverser } from '../../../bridge/reverse';
import { clipReverseState, findClip } from '../../../lib/timeline';
import { keyBurst } from '../../../state/keyBurst';
import { useReversing } from '../../../state/reversing';
import { toast } from '../../../state/toasts';
import { announce } from '../announce';
import { blockLabel, commit, editable } from './timelineActions';

/*
 * Reverse: a clip plays backward. Its footage is written backward first, in the background, to a file of its own beside
 * the project (`reverser`), its progress in the top bar's pill (`useReversing`); then the file and the turn go in as
 * one undo step (`addAssets`, `reverseClip`). A reversed clip plays forward again at once, from its own file.
 */

/** "Reversing intro.mov": the pill's words for a copy being made. */
export const reversingName = (name: string) =>
  t({
    id: 'timeline.reverse.pill',
    message: `Reversing ${name}`,
    comment: "The top bar's pill while a clip's copy is made; {name} is the clip's name",
  });

/** Reverse on clip `clipId`: forward again when it plays backward, else backward once its copy is made. */
export function reverseClip(clipId: string) {
  keyBurst.after(() => {
    const project = editable();
    const clip = project ? findClip(project, clipId) : null;
    if (!project || !clip) return;
    const label = blockLabel(project, clipId);
    const state = clipReverseState(project, clip);
    if (state === 'forward') {
      const said = t({ id: 'timeline.key.forward', message: `${label} plays forward again` });
      commit('undo.reverse', [{ type: 'reverseClip', clipId, copy: null }], said);
      return;
    }
    if (state !== 'backward' || useReversing.getState().jobs.some((j) => j.clipId === clipId)) return;
    void makeBackward(clipId, label);
  });
}

/** Writes clip `clipId` backward, then plays it from the copy; a stop says nothing, a failure says why. */
async function makeBackward(clipId: string, label: string) {
  const jobs = useReversing.getState();
  jobs.start(clipId, label);
  try {
    const copy = await reverser.make(clipId, (done) => useReversing.getState().progress(clipId, done));
    keyBurst.after(() => {
      if (!editable()) return;
      const said = t({ id: 'timeline.key.reversed', message: `${label} plays backward` });
      commit(
        'undo.reverse',
        [
          { type: 'addAssets', assets: [copy.asset] },
          { type: 'reverseClip', clipId, copy: { assetId: copy.asset.id, fromUs: copy.fromUs, toUs: copy.toUs } },
        ],
        said,
      );
    });
  } catch (error) {
    if (isCode(error, 'reverse.stopped')) {
      announce(t({ id: 'timeline.key.reverseStopped', message: 'Reversing stopped' }));
    } else {
      toast.error(error instanceof Error ? error.message : String(error));
    }
  } finally {
    useReversing.getState().end(clipId);
  }
}

/** Stops clip `clipId`'s copy being made. */
export function stopReversing(clipId: string) {
  void reverser.stop(clipId);
}
