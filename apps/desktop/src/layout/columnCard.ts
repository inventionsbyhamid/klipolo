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
 * Whether the selection's card shows, as the stores are now: a main clip, a camera, a transition or a layer the
 * inspector has fields for is selected in a project with video, and its card hasn't stepped aside for the library
 * (`useLibraryPanel`'s `aside`). Never in full screen: it is for watching. The one check the column and Command B use.
 */
export function cardWanted(): boolean {
  if (useFullScreen.getState().on) return false;
  const project = useProject.getState().snapshot?.project;
  return wantsCard(project, useSelection.getState(), useLibraryPanel.getState().aside);
}

/**
 * `cardWanted`, heard from every store it reads. Each selector asks the same check over all of them as they are now,
 * so a change to any one is heard, and the caller re-renders only when the answer changes: never per edit, nor per pick
 * while the card stays.
 */
export function useCardWanted(): boolean {
  const bySelection = useSelection(() => cardWanted());
  const byProject = useProject(() => cardWanted());
  const byPanel = useLibraryPanel(() => cardWanted());
  const byFullScreen = useFullScreen(() => cardWanted());
  return bySelection && byProject && byPanel && byFullScreen;
}

/** The selection has a card, and it stepped aside for the library (Back to library, or a view opened from it). */
function cardAside(): boolean {
  if (useFullScreen.getState().on) return false;
  const selection = useSelection.getState();
  const { aside } = useLibraryPanel.getState();
  return steppedAside(aside, selection) && wantsCard(useProject.getState().snapshot?.project, selection, null);
}

/** `cardAside`, heard from every store it reads (as `useCardWanted`). */
export function useCardAside(): boolean {
  const bySelection = useSelection(() => cardAside());
  const byProject = useProject(() => cardAside());
  const byPanel = useLibraryPanel(() => cardAside());
  const byFullScreen = useFullScreen(() => cardAside());
  return bySelection && byProject && byPanel && byFullScreen;
}

/** Whether the inspector card shows over the library: `cardWanted`, steady through a timeline press or walk. */
export function useColumnCard(): boolean {
  return useSteady(useCardWanted());
}

/**
 * A pick made from the library's own column (a file added or dropped from it, a caption or a look picked there) leaves
 * the library in place: the card steps aside as the pick lands, so working through the library never sends it away.
 * The pick is the library's when the last press or key before it went to the library (never to its card, nor to the
 * panel itself, where the keyboard waits after a card). A press anywhere else, or a key there, lets the next pick show
 * its card again. A menu or popover opened in a portal (a row's More menu) is where its trigger was: a press or key in
 * it keeps what the trigger's press said.
 */
/** A popup in a portal, outside the panel: its own items act for whatever opened it. */
const POPUP = '[data-radix-popper-content-wrapper], [role="menu"], [role="listbox"]';

export function useLibraryKeepsItsPicks(panel: RefObject<HTMLElement | null>) {
  useEffect(() => {
    let fromLibrary = false;
    const mark = (e: Event) => {
      const el = e.target instanceof Element ? e.target : null;
      if (el?.closest(POPUP)) return;
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
