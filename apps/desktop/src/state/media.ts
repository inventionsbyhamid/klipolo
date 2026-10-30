import { create } from 'zustand';

interface MediaStore {
  /** Bumped whenever a file's waveform or thumbnails change, so what draws them reloads. */
  versions: Record<string, number>;
  bump: (path: string) => void;
}

export const useMedia = create<MediaStore>((set) => ({
  versions: {},
  bump: (path) => set((s) => ({ versions: { ...s.versions, [path]: (s.versions[path] ?? 0) + 1 } })),
}));

export function useMediaVersion(path: string | null): number {
  return useMedia((s) => (path ? (s.versions[path] ?? 0) : 0));
}
