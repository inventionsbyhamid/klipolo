import { create } from 'zustand';
import { useProject } from './project';

/** A stretch of the video's time the person selected on the timeline (µs, start before end). */
export interface TimeRange {
  startUs: number;
  endUs: number;
}

interface RangeStore {
  range: TimeRange | null;
  /** Selects from `a` to `b`, in either order; an empty one clears the selection. */
  select: (a: number, b: number) => void;
  clear: () => void;
}

/**
 * The time range selected by dragging on empty lane space or the ruler, or with I and O. Delete removes
 * it from the video in one step; chat reads it as "this".
 */
export const useRange = create<RangeStore>((set) => ({
  range: null,
  select: (a, b) => {
    const startUs = Math.max(0, Math.min(a, b));
    const endUs = Math.max(a, b);
    set({ range: endUs > startUs ? { startUs, endUs } : null });
  },
  clear: () => set({ range: null }),
}));

/**
 * Clears the time range when another project opens, or the same one opens again: a range belongs to the timeline it
 * was drawn on. Returns the stop.
 */
export function clearRangeOnProjectSwitch(): () => void {
  return useProject.subscribe((s, prev) => {
    if (s.snapshot?.project.id !== prev.snapshot?.project.id || s.snapshot?.opening !== prev.snapshot?.opening)
      useRange.getState().clear();
  });
}
