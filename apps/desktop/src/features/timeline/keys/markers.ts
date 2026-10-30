import type { Item } from '../../../bindings/Item';

/** A "+N" marker: compact view's count of the layers it packed away (`lanes/LayersLane`). */
export const isMarker = (target: EventTarget | null): target is HTMLElement =>
  target instanceof HTMLElement && target.dataset.moreFrom !== undefined;

/** The "+N" marker standing for packed layer `item`, when the lanes draw it. */
export function markerFor(scroller: HTMLElement, item: Item): HTMLElement | null {
  for (const el of scroller.querySelectorAll<HTMLElement>('[data-more-from]'))
    if (item.startUs >= Number(el.dataset.moreFrom) && item.startUs < Number(el.dataset.moreTo)) return el;
  return null;
}

/** Whether a "+N" marker in `scroller` has focus. */
export const markerFocusedIn = (scroller: HTMLElement) =>
  isMarker(document.activeElement) && scroller.contains(document.activeElement);
