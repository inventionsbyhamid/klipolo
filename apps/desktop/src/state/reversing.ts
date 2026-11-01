import { create } from 'zustand';

/** A clip whose footage is being written backward: its name, as the pill says it, and how far it is (0 to 1). */
export interface Reversing {
  clipId: string;
  name: string;
  progress: number;
}

interface ReversingState {
  /** The copies being made, oldest first. */
  jobs: Reversing[];
  start: (clipId: string, name: string) => void;
  progress: (clipId: string, done: number) => void;
  end: (clipId: string) => void;
}

/** Reverse's copies being made (`reverseActions`), for the top bar's pill and the clip's commands. */
export const useReversing = create<ReversingState>((set) => ({
  jobs: [],
  start: (clipId, name) =>
    set((s) => (s.jobs.some((j) => j.clipId === clipId) ? s : { jobs: [...s.jobs, { clipId, name, progress: 0 }] })),
  progress: (clipId, done) =>
    set((s) => ({ jobs: s.jobs.map((j) => (j.clipId === clipId ? { ...j, progress: done } : j)) })),
  end: (clipId) => set((s) => ({ jobs: s.jobs.filter((j) => j.clipId !== clipId) })),
}));
