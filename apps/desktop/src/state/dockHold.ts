import { create } from 'zustand';
import { SETTLE_MS } from './bursts';

/**
 * Whether the preview keeps its dock (the card at the frame's right, or none) as it is for now: while a timeline press
 * or drag is under way (`press`), the card never comes or goes under the pointer, or over the edge frame a trim shows;
 * while the keyboard walks the lanes (`walk`), it doesn't come and go at every bare cut on the way. The preview docks
 * as the selection says once nothing holds it. The frame keeps its size either way (`stageLayout`).
 */
export const useDockHold = create<{ press: boolean; walk: boolean }>(() => ({ press: false, walk: false }));

/**
 * A timeline press: the dock holds until its gesture ends (`releaseDock`), any release or cancel of the pointer, or the
 * window losing focus (a release that goes to another app never reaches this one).
 */
export function holdDock() {
  if (useDockHold.getState().press) return;
  window.addEventListener('pointerup', releaseDock);
  window.addEventListener('pointercancel', releaseDock);
  window.addEventListener('blur', releaseDock);
  useDockHold.setState({ press: true });
}

export function releaseDock() {
  window.removeEventListener('pointerup', releaseDock);
  window.removeEventListener('pointercancel', releaseDock);
  window.removeEventListener('blur', releaseDock);
  if (useDockHold.getState().press) useDockHold.setState({ press: false });
}

let walkTimer: ReturnType<typeof setTimeout> | undefined;

/** A keyboard step along the lanes: the dock holds until the walk settles, `SETTLE_MS` after its last step. */
export function holdDockForWalk() {
  clearTimeout(walkTimer);
  walkTimer = setTimeout(() => useDockHold.setState({ walk: false }), SETTLE_MS);
  if (!useDockHold.getState().walk) useDockHold.setState({ walk: true });
}
