import { t } from '@lingui/core/macro';
import type { ReversedCopy } from '../../../bindings/ReversedCopy';
import { isCode } from '../../../bridge/call';
import { reverser } from '../../../bridge/reverse';
import { assetById, clipReverseState, findClip } from '../../../lib/timeline';
import { keyBurst } from '../../../state/keyBurst';
import { useProject } from '../../../state/project';
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
  reverse(clipId, true);
}

/**
 * Reverse's backward half only, as chat's `reverseClip` asks: clip `clipId` written backward once its copy is made, and
 * nothing when it plays backward already or a copy of it is being made.
 */
export function playBackward(clipId: string) {
  reverse(clipId, false);
}

function reverse(clipId: string, forward: boolean) {
  keyBurst.after(() => {
    const project = editable();
    const at = project ? project.timeline.main.findIndex((c) => c.id === clipId) : -1;
    if (!project || at < 0) return;
    const clip = project.timeline.main[at];
    const label = blockLabel(project, clipId);
    const state = clipReverseState(project, at);
    if (state === 'forward' && forward) {
      const said = t({ id: 'timeline.key.forward', message: `${label} plays forward again` });
      commit('undo.reverse', [{ type: 'reverseClip', clipId, copy: null }], said);
      return;
    }
    if (state !== 'backward' || useReversing.getState().jobs.some((j) => j.clipId === clipId)) return;
    const source = clip.source.type === 'asset' ? clip.source.id : '';
    void makeBackward({ projectId: project.id, clipId, source }, label);
  });
}

// Chat's `reverseClip` writes a clip backward through the store (`useReversing`'s `backward`): the same job, pill and
// undo step as the clip menu's Reverse.
useReversing.setState({ backward: playBackward });

/** What a copy is made for: the project, the clip, and the file the clip played when it was asked. */
interface Asked {
  projectId: string;
  clipId: string;
  source: string;
}

/**
 * Whether `copy` still fits what was asked: the same project, open and editable, and the clip there, playing the same
 * file forward over a stretch the copy holds. Anything else (another project opened, the clip went, was trimmed past
 * the copy or reversed another way) and the copy isn't used.
 */
function stillFits(asked: Asked, copy: ReversedCopy): boolean {
  const project = editable();
  if (!project || project.id !== asked.projectId) return false;
  const clip = findClip(project, asked.clipId);
  return (
    !!clip &&
    clip.source.type === 'asset' &&
    clip.source.id === asked.source &&
    !clip.reversed &&
    copy.fromUs <= clip.sourceInUs &&
    clip.sourceOutUs <= copy.toUs
  );
}

/**
 * Writes the clip backward, then plays it from the copy once the copy still fits what was asked (`stillFits`); a copy
 * that doesn't, or whose edit is refused, is deleted. A stop says nothing more, a failure says why.
 */
async function makeBackward(asked: Asked, label: string) {
  const { clipId } = asked;
  const jobs = useReversing.getState();
  jobs.start(clipId, label);
  try {
    const copy = await reverser.make(clipId, (done) => useReversing.getState().progress(clipId, done));
    const discard = () => void reverser.discard(copy.asset.path);
    // Another project is open: the copy is never put in it, and nothing is said there.
    if (useProject.getState().snapshot?.project.id !== asked.projectId) return discard();
    keyBurst.after(() => {
      if (!stillFits(asked, copy)) {
        discard();
        toast.info(
          t({
            id: 'timeline.reverse.changed',
            message: `${label} changed while it was reversed, so it plays as before.`,
            comment: "A clip edited or removed while its footage was written backward; {label} is the clip's name",
          }),
        );
        return;
      }
      const said = t({ id: 'timeline.key.reversed', message: `${label} plays backward` });
      // A clip with sound whose copy has none (too long, or its sound unreadable) is told so once it is in.
      const project = editable();
      const silent = !copy.asset.hasAudio && !!project && !!assetById(project, asked.source)?.hasAudio;
      const tell = () =>
        toast.info(
          t({
            id: 'timeline.reverse.silent',
            message: `${label} plays backward without sound, because its sound couldn't be reversed.`,
            comment: "A clip written backward whose sound couldn't be; {label} is the clip's name",
          }),
        );
      commit(
        'undo.reverse',
        [
          { type: 'addAssets', assets: [copy.asset] },
          { type: 'reverseClip', clipId, copy: { assetId: copy.asset.id, fromUs: copy.fromUs, toUs: copy.toUs } },
        ],
        said,
        silent ? tell : undefined,
        discard,
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
