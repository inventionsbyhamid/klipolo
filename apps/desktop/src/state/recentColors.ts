import { create } from 'zustand';
import { createJSONStorage, persist } from 'zustand/middleware';

/** How many custom colours "Recent" keeps. */
export const RECENT_MAX = 8;

/** A saved colour: `#RRGGBB`, or `#RRGGBBAA`. */
const SAVED = /^#[0-9A-F]{6}([0-9A-F]{2})?$/i;

interface RecentColors {
  /** The custom colours picked last, newest first: uppercase `#RRGGBB`, each once, at most `RECENT_MAX`. */
  recent: string[];
  /**
   * `color` goes first, in capitals and without an alpha (Recent holds colours: a pick keeps the setting's opacity);
   * a repeat moves there; the list keeps `RECENT_MAX`.
   */
  push(color: string): void;
}

/** A colour as Recent keeps it: uppercase `#RRGGBB`. */
const asRecent = (color: string) => color.slice(0, 7).toUpperCase();

/** A saved list as the store keeps it: its colours (nothing else), as `asRecent`, each once, at most `RECENT_MAX`. */
function savedRecent(saved: unknown): string[] {
  if (!Array.isArray(saved)) return [];
  const colors = saved.filter((c): c is string => typeof c === 'string' && SAVED.test(c));
  return [...new Set(colors.map(asRecent))].slice(0, RECENT_MAX);
}

/**
 * The person's last custom colour picks, for the colour popover's "Recent", the same in every project
 * and kept across launches (`klipolo-colors`). A pick writes it once (a drag on release, keys once they settle), and
 * pushing the colour already first writes nothing. A saved value of the wrong shape reads as none, never a crash.
 */
export const useRecentColors = create<RecentColors>()(
  persist(
    (set, get) => ({
      recent: [],
      push: (color) => {
        const hex = asRecent(color);
        const { recent } = get();
        if (recent[0] === hex) return;
        set({ recent: [hex, ...recent.filter((c) => c !== hex)].slice(0, RECENT_MAX) });
      },
    }),
    {
      name: 'klipolo-colors',
      storage: createJSONStorage(() => localStorage),
      partialize: ({ recent }) => ({ recent }),
      merge: (saved, current) => ({
        ...current,
        recent: savedRecent(
          typeof saved === 'object' && saved !== null ? (saved as { recent?: unknown }).recent : undefined,
        ),
      }),
    },
  ),
);
