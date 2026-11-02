import { useEffect, useState, type RefObject } from 'react';
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

/**
 * A pick made from the library's own column (a file added or dropped from it, a caption or a look picked there) leaves
 * the library in place: the card steps aside as the pick lands, so working through the library never sends it away.
 * The pick is the library's when the last press or key before it went to the library (never to its card, nor to the
 * panel itself, where the keyboard waits after a card). A press anywhere else, or a key there, lets the next pick show
 * its card again.
 */
export function useLibraryKeepsItsPicks(panel: RefObject<HTMLElement | null>) {
  useEffect(() => {
    let fromLibrary = false;
    const mark = (e: Event) => {
      const el = e.target instanceof Element ? e.target : null;
      const own = panel.current;
      fromLibrary = !!el && !!own && el !== own && own.contains(el) && el.closest('.inspector') === null;
    };
    window.addEventListener('pointerdown', mark, true);
    window.addEventListener('keydown', mark, true);
    const off = useSelection.subscribe((s, prev) => {
      const picked = s.selectedId !== null && (s.selectedIds !== prev.selectedIds || s.selectedId !== prev.selectedId);
      if (fromLibrary && picked) useLibraryPanel.getState().stepAside();
    });
    return () => {
      window.removeEventListener('pointerdown', mark, true);
      window.removeEventListener('keydown', mark, true);
      off();
    };
  }, [panel]);
}
