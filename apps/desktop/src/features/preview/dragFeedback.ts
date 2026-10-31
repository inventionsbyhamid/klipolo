import { create } from 'zustand';
import type { Guide } from '../../lib/geometry';

interface DragFeedback {
  /** A layer is being moved, resized or turned on the canvas (9:16 shows its safe area meanwhile). */
  dragging: boolean;
  /** The snap lines a moving layer sits on. */
  guides: Guide[];
}

const NONE: Guide[] = [];

/**
 * What a drag shows around the layer, kept out of React's editor tree: only the guides and the safe area re-render,
 * and only when this changes (the guides at most when a snap engages or lets go, never per pointer event).
 */
export const useDragFeedback = create<DragFeedback>(() => ({ dragging: false, guides: NONE }));

export function setDragging(dragging: boolean) {
  if (useDragFeedback.getState().dragging !== dragging)
    useDragFeedback.setState(dragging ? { dragging } : { dragging, guides: NONE });
}

export function showGuides(guides: Guide[]) {
  const now = useDragFeedback.getState().guides;
  const same =
    now.length === guides.length && now.every((g, i) => g.axis === guides[i].axis && g.at === guides[i].at);
  if (!same) useDragFeedback.setState({ guides: guides.length ? guides : NONE });
}
