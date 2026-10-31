import { create } from 'zustand';
import type { FormatKind } from '../bindings/FormatKind';

/**
 * The view settings the editor reads: the timeline's zoom and snapping, and the format the preview shows. Panel
 * widths and heights live in Klipolo's layout (the CSS grid's variables), not here. The timeline always shows its four
 * lanes (`timelineExpanded` is never on).
 */
interface UiStore {
  /** Timeline zoom: pixels per second of video. */
  pxPerSecond: number;
  setPxPerSecond: (pxPerSecond: number) => void;
  /** Dragged blocks snap to the playhead, cuts and edges. */
  snapping: boolean;
  setSnapping: (snapping: boolean) => void;
  readonly timelineExpanded: false;
  /** The format the preview shows; null: the project's primary one. */
  previewFormat: FormatKind | null;
  setPreviewFormat: (format: FormatKind | null) => void;
  /** Bumped by Command K: the chat panel opens and puts the cursor in its composer. */
  chatFocus: number;
  focusChat: () => void;
}

export const useUi = create<UiStore>((set) => ({
  pxPerSecond: 40,
  setPxPerSecond: (pps) => set({ pxPerSecond: Math.min(2400, Math.max(0.02, pps)) }),
  snapping: true,
  setSnapping: (snapping) => set({ snapping }),
  timelineExpanded: false,
  previewFormat: null,
  setPreviewFormat: (previewFormat) => set({ previewFormat }),
  chatFocus: 0,
  focusChat: () => set((s) => ({ chatFocus: s.chatFocus + 1 })),
}));
