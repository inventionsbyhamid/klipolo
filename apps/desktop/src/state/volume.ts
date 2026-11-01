import { create } from 'zustand';
import { engine } from '../bridge/engine';

/** Where the player bar's volume is kept on this Mac (local storage). */
const KEPT = 'klipolo.volume';

interface Volume {
  /** The slider, 0 to 1, in hundredths. */
  level: number;
  /** Mute is on: silent, the level kept for Unmute. */
  muted: boolean;
}

interface VolumeStore extends Volume {
  /** Sets the slider (held to 0 to 1); moving it unmutes. */
  setLevel: (level: number) => void;
  toggleMute: () => void;
}

/** The volume as the person left it; full and not muted when nothing was kept, or storage is refused. */
function kept(): Volume {
  try {
    const kept = JSON.parse(localStorage.getItem(KEPT) ?? 'null') as Partial<Volume> | null;
    const level = kept?.level;
    if (typeof level === 'number' && level >= 0 && level <= 1) return { level, muted: kept?.muted === true };
  } catch {
    // Unreadable: full volume.
  }
  return { level: 1, muted: false };
}

function keep({ level, muted }: Volume) {
  try {
    localStorage.setItem(KEPT, JSON.stringify({ level, muted }));
  } catch {
    // Storage refused: the volume holds for this session.
  }
}

/**
 * The player bar's volume: how loud the preview plays for the person, never what an export holds. Kept on this Mac.
 */
export const useVolume = create<VolumeStore>((set, get) => ({
  ...kept(),
  setLevel: (level) => {
    set({ level: Math.round(Math.min(1, Math.max(0, level)) * 100) / 100, muted: false });
    keep(get());
  },
  toggleMute: () => {
    set((s) => ({ muted: !s.muted }));
    keep(get());
  },
}));

/** The gain the preview plays at: the slider squared, as loudness is heard on a curve (half way is a quarter); 0 muted. */
export function heardVolume({ level, muted }: Volume): number {
  return muted ? 0 : level * level;
}

/** Tells the engine the volume (as it attaches: a new engine starts at full volume). */
export function sendVolume() {
  void engine.setVolume(heardVolume(useVolume.getState())).catch(() => undefined);
}

/** Tells the engine each change of the volume. Returns the stop. */
export function followVolume(): () => void {
  return useVolume.subscribe((s, prev) => {
    if (heardVolume(s) !== heardVolume(prev)) sendVolume();
  });
}
