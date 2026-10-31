import { select, t } from '@lingui/core/macro';
import type { Clip } from '../../../bindings/Clip';
import type { ContribId } from '../../../bindings/ContribId';
import type { Op } from '../../../bindings/Op';
import type { ParamValue } from '../../../bindings/ParamValue';
import type { Project } from '../../../bindings/Project';
import type { Transition } from '../../../bindings/Transition';
import { contribLabel, isInstalled } from '../../../lib/contrib';
import { labelText } from '../../../lib/labels';
import { sourceLabel } from '../../../lib/timeline';
import { maxTransitionUs, splitDuration, transitionLengthUs, TRANSITION_DISSOLVE } from '../../../lib/transitions';
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
 * What the transition commands, a cut's marker and the transition card do: each builds one `setTransition`
 * and commits it as one undo step through `useProject.apply`, the path every edit takes.
 */

/** The commands' names, which are also their undo labels (`transition.add`, `transition.remove`). */
export const transitionLabels = {
  add: () => labelText('transition.add'),
  remove: () => labelText('transition.remove'),
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
 * longest that fits, unless the type (`effect`) or the length (`durationUs`, held within the footage on the frame
 * grid) is given. A cut that has one already selects it. Then the new one is selected, and focus on a block moves
 * to its cut.
 */
export function addTransition(clipId: string, options: { effect?: ContribId; durationUs?: number } = {}) {
  keyBurst.after(() => {
    const project = editable();
    const i = project ? cutIndex(project, clipId) : -1;
    if (!project || i < 0) return;
    if (project.timeline.main[i].transitionOut) return pickTransition(clipId);
    const fps = project.settings.fps;
    const ops = addTransitionOps(project, i, fps);
    const op = ops[0];
    if (op?.type !== 'setTransition' || !op.transition) return noRoomToast();
    const effect = options.effect ?? TRANSITION_DISSOLVE;
    const durationUs =
      options.durationUs === undefined
        ? op.transition.durationUs
        : transitionLengthUs(options.durationUs, maxTransitionUs(project, i), fps);
    const { before, after } = cutLabels(project, i);
    const added =
      effect === TRANSITION_DISSOLVE
        ? t({ id: 'transition.added', message: `Added a dissolve between ${before} and ${after}` })
        : t({
            id: 'transition.addedNamed',
            message: `Added a transition between ${before} and ${after}`,
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
    commit('transition.add', [transitionOp(clipId, { effect, durationUs, params: {} })], said, () => {
      useSelection.getState().select(transitionSelection(clipId));
      if (blockFocused()) useTimelineFocus.getState().focus(cutId(clipId), true);
    });
  });
}

/** Remove transition (Delete with one selected, the card's Remove): clip `clipId`'s transition out. */
export function removeTransition(clipId: string) {
  keyBurst.after(() => {
    const project = editable();
    const i = project ? cutIndex(project, clipId) : -1;
    if (!project || i < 0 || !project.timeline.main[i].transitionOut) return;
    const { before, after } = cutLabels(project, i);
    commit(
      'transition.remove',
      [transitionOp(clipId, null)],
      t({ id: 'transition.removed', message: `Removed the transition between ${before} and ${after}` }),
    );
  });
}

/**
 * Change transition (`transition-set`): clip `clipId`'s transition with a new type (its settings start from the
 * new type's defaults), length (held within the footage on the frame grid) or settings; `reset` keys go back to their
 * defaults. One undo step.
 */
export function changeTransition(
  clipId: string,
  change: { effect?: ContribId; durationUs?: number; params?: Record<string, ParamValue>; reset?: string[] },
) {
  keyBurst.after(() => {
    const project = editable();
    const i = project ? cutIndex(project, clipId) : -1;
    const was = i >= 0 ? project!.timeline.main[i].transitionOut : undefined;
    if (!project || !was) return;
    const retyped = change.effect !== undefined && change.effect !== was.effect;
    const params: Record<string, ParamValue> = { ...(retyped ? {} : was.params), ...change.params };
    for (const key of change.reset ?? []) delete params[key];
    const durationUs =
      change.durationUs === undefined
        ? was.durationUs
        : transitionLengthUs(change.durationUs, maxTransitionUs(project, i), project.settings.fps);
    if (durationUs === 0) return noRoomToast();
    const lengthOnly = change.durationUs !== undefined && !retyped && !change.params;
    const label = lengthOnly ? 'transition.length.undo' : 'transition.change.undo';
    const { before, after } = cutLabels(project, i);
    commit(
      label,
      [transitionOp(clipId, { effect: change.effect ?? was.effect, durationUs, params })],
      t({ id: 'transition.changed', message: `Changed the transition between ${before} and ${after}` }),
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
 * The cut nearest `us` (the clip before it, by index): a binary search over the clips' ends. -1 with fewer than 2
 * clips.
 */
export function nearestCut(index: MainIndex, us: number): number {
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
