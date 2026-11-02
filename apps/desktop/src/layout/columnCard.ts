import { useState } from 'react';
import type { Project } from '../bindings/Project';
import { inspectedOf } from '../features/inspector/Inspector';
import { inspectedTransition } from '../features/inspector/TransitionInspector';
import { useDockHold } from '../state/dockHold';
import { useFullScreen } from '../state/fullScreen';
import { steppedAside, useLibraryPanel } from '../state/libraryPanel';
import { useProject } from '../state/project';
import { useSelection } from '../state/selection';

/**
 * `wanted`, except while a timeline press or drag, or a keyboard walk along the lanes, holds the column
 * (`state/dockHold`); then what it was before, so the card doesn't come and go under the pointer or at every bare cut,
 * and a folded column doesn't open and fold on the way. It follows `wanted` again once the hold ends.
 */
function useSteady(wanted: boolean): boolean {
  const held = useDockHold((s) => s.press || s.walk);
  const [shown, setShown] = useState(wanted);
  if (!held && shown !== wanted) setShown(wanted);
  return held ? shown : wanted;
}

type Picked = Pick<ReturnType<typeof useSelection.getState>, 'selectedId' | 'selectedIds'>;
type Aside = ReturnType<typeof useLibraryPanel.getState>['aside'];

/** Whether `project` has video and a card for the selection `s`, and that card hasn't stepped aside. */
function wantsCard(project: Project | undefined, s: Picked, aside: Aside): boolean {
  if (!project || project.timeline.main.length === 0 || steppedAside(aside, s)) return false;
  return inspectedOf(project, s.selectedId) !== null || inspectedTransition(project, s.selectedId) !== null;
}

/**
 * Whether the left column shows the inspector card over the library: while a main clip, a camera, a transition or a
 * layer the inspector has fields for is selected in a project with video, unless its card stepped aside for the
 * library (`useLibraryPanel`'s `aside`), never in full screen, and steady through a timeline press or walk
 * (`useSteady`). The library re-renders only when the answer changes: never per edit, nor per pick while the card
 * stays (each store's selector reads the other's state as it is now, so either one's change is heard).
 */
export function useColumnCard(): boolean {
  const aside = useLibraryPanel((s) => s.aside);
  const bySelection = useSelection((s) => wantsCard(useProject.getState().snapshot?.project, s, aside));
  const byProject = useProject((p) => wantsCard(p.snapshot?.project, useSelection.getState(), aside));
  // Full screen is for watching: the card stays out of the way until it ends.
  const full = useFullScreen((s) => s.on);
  return useSteady(bySelection && byProject) && !full;
}
