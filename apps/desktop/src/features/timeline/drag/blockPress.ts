import type { PointerEvent, RefObject } from 'react';
import type { Edge } from '../../../bindings/Edge';
import { findItem, itemEndUs, itemLabel, showItemAtUs, sourceLabel } from '../../../lib/timeline';
import { holdDock } from '../../../state/dockHold';
import { usePlayback } from '../../../state/playback';
import { keyBurst } from '../../../state/keyBurst';
import { useProject } from '../../../state/project';
import { useSelection } from '../../../state/selection';
import { useUi } from '../../../state/ui';
import { mainIndex } from '../model/index';
import { pxToUs } from '../scale';
import { timelineDragView } from './dragView';
import { blockElement } from './registry';
import type { Gesture } from './session';
import { trimGesture } from './trimFrame';
import { onTrimHandle, pressBlock, pressTrim } from './useBlockDrag';

/** On a Mac, ⌃-click is a right-click: the block's menu opens (`BlockMenu`), and the press neither picks nor drags. */
const MAC = typeof navigator !== 'undefined' && /^Mac/.test(navigator.platform);

/**
 * Selects block `id`. Outside it, the playhead goes where it shows whole (past a title's fade-in); inside, it stays, so
 * S splits where the user was.
 */
export function pickBlock(id: string) {
  const project = useProject.getState().snapshot?.project;
  if (!project) return;
  const found = findItem(project, id);
  const i = found ? -1 : project.timeline.main.findIndex((clip) => clip.id === id);
  if (!found && i < 0) return;
  const index = mainIndex(project.timeline.main);
  const [startUs, endUs, showAtUs] = found
    ? [found.item.startUs, itemEndUs(found.item), showItemAtUs(found.item, project.settings.fps)]
    : [index.starts[i], index.ends[i], index.starts[i]];
  useSelection.getState().select(id);
  const { playheadUs, seek } = usePlayback.getState();
  if (playheadUs < startUs || playheadUs >= endUs) seek(showAtUs);
}

/**
 * The blocks' press handlers, for the timeline whose scroller, snap guide's parts and tip these refs hold. They read
 * the project as it is at the press, so their identities never change (`memo` blocks never re-render for them).
 */
export function blockPressHandlers(
  scrollRef: RefObject<HTMLElement | null>,
  guideRefs: readonly RefObject<HTMLElement | null>[],
  tipRef: RefObject<HTMLElement | null>,
) {
  /** A press picks, then a move starts with the project as it is now. */
  const onPress = (e: PointerEvent<HTMLElement>, id: string) => {
    const scroller = scrollRef.current;
    const project = useProject.getState().snapshot?.project;
    // An edge's press is the trim's (`onTrimPress`, which ran first).
    if (e.button !== 0 || (MAC && e.ctrlKey) || !scroller || !project || onTrimHandle(e)) return;
    // A nudge still settling goes in first: the gesture builds on where it put things.
    keyBurst.flush();
    // The pressed block takes focus (WebKit's mousedown would otherwise blur the focused one: buttons aren't
    // mouse-focusable on a Mac), so focus never drops to the page; and a drag selects no text.
    e.preventDefault();
    e.currentTarget.focus({ preventScroll: true });
    // The move snaps to the playhead where the user had it, not where this pick puts it (the block's own start).
    const playheadUs = usePlayback.getState().playheadUs;
    // The preview docks the picked block's card once the press is over, not under the pointer.
    holdDock();
    pickBlock(id);
    const found = findItem(project, id);
    const { main } = project.timeline;
    let gesture: Gesture;
    if (found) {
      const { item, track } = found;
      const label = itemLabel(project, item);
      gesture = { kind: 'item', itemId: id, label, trackId: track.id, trackKind: track.kind };
    } else {
      const i = main.findIndex((clip) => clip.id === id);
      if (i < 0) return;
      const atPx = e.clientX - e.currentTarget.getBoundingClientRect().left;
      const grabUs = mainIndex(main).starts[i] + pxToUs(atPx, useUi.getState().pxPerSecond);
      gesture = { kind: 'clip', clipId: id, label: sourceLabel(project, main[i].source), grabUs };
    }
    const view = timelineDragView(scroller, found?.track.id ?? null, guideRefs, tipRef);
    pressBlock(e, gesture, view, playheadUs);
  };

  /**
   * A press on a block's edge selects it, leaving the playhead where it is (trimming to it is the usual trim), and
   * trims that edge (`drag/trimFrame`).
   */
  const onTrimPress = (e: PointerEvent<HTMLElement>, id: string, edge: Edge) => {
    const scroller = scrollRef.current;
    const project = useProject.getState().snapshot?.project;
    const block = blockElement(id);
    const gesture = project ? trimGesture(project, id, edge) : null;
    if (e.button !== 0 || (MAC && e.ctrlKey) || !scroller || !block || !gesture) return;
    keyBurst.flush();
    e.preventDefault();
    block.focus({ preventScroll: true });
    holdDock();
    useSelection.getState().select(id);
    pressTrim(e, block, gesture, timelineDragView(scroller, null, guideRefs, tipRef));
  };

  return { onPick: pickBlock, onPress, onTrimPress };
}
