import { create } from 'zustand';

/**
 * Whether the preview keeps its dock (the card beside the frame, or none) as it is for now: while a timeline press or
 * drag is under way, the frame never changes size under the pointer, or under the edge frame a trim shows. The preview
 * docks as the selection says once nothing holds it.
 */
export const useDockHold = create<{ press: boolean }>(() => ({ press: false }));

/** A timeline press: the dock holds until its gesture ends (`releaseDock`), or any release or cancel of the pointer. */
export function holdDock() {
  if (useDockHold.getState().press) return;
  window.addEventListener('pointerup', releaseDock);
  window.addEventListener('pointercancel', releaseDock);
  useDockHold.setState({ press: true });
}

export function releaseDock() {
  window.removeEventListener('pointerup', releaseDock);
  window.removeEventListener('pointercancel', releaseDock);
  if (useDockHold.getState().press) useDockHold.setState({ press: false });
}
