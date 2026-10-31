import { select, t } from '@lingui/core/macro';
import type { Clip } from '../../../bindings/Clip';
import type { Op } from '../../../bindings/Op';
import type { Project } from '../../../bindings/Project';
import type { Transition } from '../../../bindings/Transition';
import { contribLabel, isInstalled } from '../../../lib/contrib';
import { labelText } from '../../../lib/labels';
import { sourceLabel } from '../../../lib/timeline';
import { splitDuration } from '../../../lib/transitions';
import { keyBurst } from '../../../state/keyBurst';
import { usePlayback } from '../../../state/playback';
import { useProject } from '../../../state/project';
import { transitionSelection, useSelection } from '../../../state/selection';
import { toast } from '../../../state/toasts';
import { useTimelineFocus } from '../../../state/timelineFocus';
import { mainIndex, type MainIndex } from '../model/index';
import { addTransitionOps } from './keyOps';
import { cutId } from './laneOrder';
import { blockFocused, commit, editable } from './timelineActions';

/*
 * What Add transition, a cut's marker and Delete do with transitions: each builds one `setTransition` and
 * commits it as one undo step through `useProject.apply`, the path every edit takes. The transition card builds its
 * own at its turn (`transitionNow`, `transitionOp`).
 */

/** Add transition's name, which is also its undo label (`transition.add`), and a bare cut's tip. */
export const transitionLabels = {
  add: () => labelText('transition.add'),
  /** Why a cut can't take one: the tip of its marker. */
  noRoom: () =>
    t({
      id: 'transition.noRoom',
      message: 'Not enough extra footage here for a transition',
      comment: 'A cut whose clips have no footage past their ends to blend',
    }),
};

/** The op that sets (or, with null, removes) clip `clipId`'s transition into the next clip. */
export const transitionOp = (clipId: string, transition: Transition | null): Op => ({
  type: 'setTransition',
  clipId,
  transition,
});

/** A transition's name: its contribution's ("Dissolve"), or "Missing transition" when this build lacks it. */
export const transitionName = (transition: Transition) => contribLabel(transition.effect, 'transition');

/** What a transition is, named whole: "Dissolve transition"; one this build lacks is just "Missing transition". */
export function transitionKind(transition: Transition): string {
  const name = transitionName(transition);
  return isInstalled(transition.effect)
    ? t({ id: 'transition.kind', message: `${name} transition`, comment: '{name} is e.g. Dissolve' })
    : name;
}

/** The fades at a cut that a transition there replaces (the model drops them): the fade out, in, both, or none. */
export type FadesReplaced = 'none' | 'out' | 'in' | 'both';

/** Which fades at the cut after clip `i` (`main[i]`'s fade out, `main[i + 1]`'s fade in) a transition replaces. */
export function fadesReplaced(main: readonly Clip[], i: number): FadesReplaced {
  const out = main[i].fadeOutUs > 0;
  const into = (main[i + 1]?.fadeInUs ?? 0) > 0;
  return out && into ? 'both' : out ? 'out' : into ? 'in' : 'none';
}

/** An Add transition button's tip, saying which fades it replaces. */
export function addTip(replaces: FadesReplaced): string {
  return t({
    id: 'transition.add.tip',
    message: select(replaces, {
      out: 'Add transition (replaces the fade out)',
      in: 'Add transition (replaces the fade in)',
      both: 'Add transition (replaces both fades)',
      other: 'Add transition',
    }),
    comment: 'A cut’s + button; a transition replaces the clips’ fades at that cut',
  });
}

/** The sentence an added transition's announcement ends with when it replaced fades; '' when it didn't. */
function replacedNote(replaces: FadesReplaced): string {
  if (replaces === 'none') return '';
  return t({
    id: 'transition.replacesFades',
    message: select(replaces, {
      out: 'It replaces the fade out.',
      in: 'It replaces the fade in.',
      other: 'It replaces both fades.',
    }),
    comment: 'Said after adding a transition at a cut that had fades: the transition takes their place',
  });
}

/** The names of the clips either side of the cut after clip `i`. */
export function cutLabels(project: Project, i: number): { before: string; after: string } {
  const { main } = project.timeline;
  return { before: sourceLabel(project, main[i].source), after: sourceLabel(project, main[i + 1].source) };
}

const noRoomToast = () =>
  toast.info(
    t({
      id: 'transition.noRoom.toast',
      message: 'Not enough extra footage here for a transition. Trim the clips at this cut to make room.',
    }),
  );

/** The clip `clipId` with a clip after it, in `project` as it is now: its index, or -1. */
function cutIndex(project: Project, clipId: string): number {
  const main = project.timeline.main;
  const i = main.findIndex((clip) => clip.id === clipId);
  return i >= 0 && i + 1 < main.length ? i : -1;
}

/**
 * Clip `clipId`'s transition as the project has it now (a change still landing included), with the project and the
 * clip's index: what the transition card builds its whole `setTransition` from when it goes in.
 */
export function transitionNow(clipId: string): { project: Project; index: number; transition: Transition } | null {
  const project = useProject.getState().snapshot?.project;
  const index = project ? cutIndex(project, clipId) : -1;
  const transition = index >= 0 ? project!.timeline.main[index].transitionOut : undefined;
  return project && transition ? { project, index, transition } : null;
}

/**
 * Selects clip `clipId`'s transition (a click on its chip, Enter on its cut); the playhead goes to where it starts
 * unless it is already inside it, so Space plays it.
 */
export function pickTransition(clipId: string) {
  const project = useProject.getState().snapshot?.project;
  const i = project ? cutIndex(project, clipId) : -1;
  const transition = i >= 0 ? project!.timeline.main[i].transitionOut : undefined;
  if (!project || !transition) return;
  useSelection.getState().select(transitionSelection(clipId));
  const cut = mainIndex(project.timeline.main).ends[i];
  const [before, after] = splitDuration(transition.durationUs);
  const { playheadUs, seek } = usePlayback.getState();
  if (playheadUs < cut - before || playheadUs >= cut + after) seek(cut - before);
}

/**
 * Add transition (⌘T, a cut's +, a clip's menu): from clip `clipId` into the next, a dissolve of half a second or the
 * longest that fits, on the frame grid. A cut that has one already selects it. Then the new one is selected, and
 * focus on a block moves to its cut.
 */
export function addTransition(clipId: string) {
  keyBurst.after(() => {
    const project = editable();
    const i = project ? cutIndex(project, clipId) : -1;
    if (!project || i < 0) return;
    if (project.timeline.main[i].transitionOut) return pickTransition(clipId);
    const ops = addTransitionOps(project, i, project.settings.fps);
    if (ops.length === 0) return noRoomToast();
    const { before, after } = cutLabels(project, i);
    const added = t({
      id: 'transition.added',
      message: `Added a dissolve between ${before} and ${after}`,
      comment: 'Screen-reader note; {before} and {after} are the clips’ names',
    });
    const note = replacedNote(fadesReplaced(project.timeline.main, i));
    const said = note
      ? t({
          id: 'transition.added.note',
          message: `${added}. ${note}`,
          comment: 'Screen-reader note: {added} says what went in, {note} which fades it replaced',
        })
      : added;
    commit('transition.add', ops, said, () => {
      useSelection.getState().select(transitionSelection(clipId));
      if (blockFocused()) useTimelineFocus.getState().focus(cutId(clipId), true);
    });
  });
}

/**
 * Remove transition (Delete with one selected, the card's Remove): clip `clipId`'s transition out. `focusCut`: focus
 * goes to its cut once it has gone (the card's Remove, which goes with it), never to the page.
 */
export function removeTransition(clipId: string, { focusCut = false }: { focusCut?: boolean } = {}) {
  keyBurst.after(() => {
    const project = editable();
    const i = project ? cutIndex(project, clipId) : -1;
    if (!project || i < 0 || !project.timeline.main[i].transitionOut) return;
    const { before, after } = cutLabels(project, i);
    commit(
      'transition.remove',
      [transitionOp(clipId, null)],
      t({ id: 'transition.removed', message: `Removed the transition between ${before} and ${after}` }),
      () => {
        if (focusCut) useTimelineFocus.getState().focus(cutId(clipId), true);
      },
    );
  });
}

/** Enter on a cut: its transition is selected; a bare cut with room for one gets one; one without says why. */
export function enterCut(project: Project, clipId: string) {
  const i = cutIndex(project, clipId);
  if (i < 0) return;
  if (project.timeline.main[i].transitionOut) return pickTransition(clipId);
  if (addTransitionOps(project, i, project.settings.fps).length > 0) return addTransition(clipId);
  noRoomToast();
}

/**
 * The clip before the cut nearest `us`, by index: a binary search over the clips' ends. -1 with fewer than 2 clips.
 * (`model/dropOps`' `nearestCut` is where a dropped clip goes, the video's start and end included.)
 */
export function clipBeforeNearestCut(index: MainIndex, us: number): number {
  const cuts = index.ends.length - 1;
  if (cuts < 1) return -1;
  let lo = 0;
  let hi = cuts;
  while (lo < hi) {
    const mid = (lo + hi) >>> 1;
    if (index.ends[mid] < us) lo = mid + 1;
    else hi = mid;
  }
  // `lo` is the first cut at or after `us` (or `cuts`, past the last): it or the one before it.
  if (lo >= cuts) return cuts - 1;
  return lo > 0 && us - index.ends[lo - 1] <= index.ends[lo] - us ? lo - 1 : lo;
}
