import { create } from 'zustand';
import type { FormatKind } from '../bindings/FormatKind';
import { SAFE_AREA_IDS, type SafeAreaId } from '../lib/safeAreas';

/**
 * The view settings the editor reads: the timeline's zoom and snapping, and the format the preview shows. Panel
 * widths and heights live in Klipolo's layout (the CSS grid's variables), not here. The timeline always shows its four
 * lanes (`timelineExpanded` is never on).
 */
interface UiStore {
  /** Timeline zoom: pixels per second of video. Any zoom pays a fit still owed (`fitOwed`): the person's stays. */
  pxPerSecond: number;
  setPxPerSecond: (pxPerSecond: number) => void;
  /**
   * The timeline fits the whole video at its next chance (`TimelineView`): owed as a project opens, and kept while it
   * has no clips, so an empty project fits once its first clips come in. Never after the person zooms.
   */
  fitOwed: boolean;
  oweFit: () => void;
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
  /** The 9:16 safe area the preview shows (the safe area menu), or off. Kept on this Mac, with `lastSafeArea`. */
  safeArea: SafeAreaId;
  /** The area shown last (what a drag in 9:16 shows and snaps to while it's off). */
  lastSafeArea: SafeAreaId;
  setSafeArea: (area: SafeAreaId) => void;
  /** The Library's filter pill. */
  libraryTab: LibraryTab;
  setLibraryTab: (tab: LibraryTab) => void;
  /**
   * Klipolo makes editing copies of slow files by itself (on unless the person turns it off; kept on this Mac). Sent to
   * Rust as the editor opens, each time the engine attaches (`useEngineSession`) and when the person flips the switch
   * (`useMediaJobs.setAuto`).
   */
  autoCopies: boolean;
  setAutoCopies: (on: boolean) => void;
}

/** The Library's tabs: your clips, music, sounds and images. */
export type LibraryTab = 'clips' | 'music' | 'sounds' | 'images';

/** Where the Auto copies switch is kept (local storage: `off` when the person turned it off). */
const AUTO_COPIES = 'klipolo.autoCopies';

/** Where the safe area choice is kept (local storage): the area shown and the one chosen last, as JSON. */
const SAFE_AREA = 'klipolo.safeArea';

type SafeAreaKept = Pick<UiStore, 'safeArea' | 'lastSafeArea'>;

/** The safe area as the person left it: each part as kept when it's one Klipolo knows, else off and all platforms. */
function keptSafeArea(): SafeAreaKept {
  const kept: SafeAreaKept = { safeArea: 'off', lastSafeArea: 'all' };
  try {
    const saved: unknown = JSON.parse(localStorage.getItem(SAFE_AREA) ?? 'null');
    if (!saved || typeof saved !== 'object') return kept;
    const { safeArea, lastSafeArea } = saved as Record<string, unknown>;
    if (SAFE_AREA_IDS.includes(safeArea as SafeAreaId)) kept.safeArea = safeArea as SafeAreaId;
    if (lastSafeArea !== 'off' && SAFE_AREA_IDS.includes(lastSafeArea as SafeAreaId))
      kept.lastSafeArea = lastSafeArea as SafeAreaId;
  } catch {
    // Unreadable or refused: as the first time.
  }
  return kept;
}

function keepSafeArea(kept: SafeAreaKept) {
  try {
    localStorage.setItem(SAFE_AREA, JSON.stringify(kept));
  } catch {
    // Storage refused: the choice holds for this session.
  }
}

/** The Auto copies switch as the person left it; on when nothing was kept, or storage is refused. */
function keptAutoCopies(): boolean {
  try {
    return localStorage.getItem(AUTO_COPIES) !== 'off';
  } catch {
    return true;
  }
}

export const useUi = create<UiStore>((set) => ({
  pxPerSecond: 40,
  setPxPerSecond: (pps) => set({ pxPerSecond: Math.min(2400, Math.max(0.02, pps)), fitOwed: false }),
  fitOwed: false,
  oweFit: () => set({ fitOwed: true }),
  snapping: true,
  setSnapping: (snapping) => set({ snapping }),
  timelineExpanded: false,
  previewFormat: null,
  setPreviewFormat: (previewFormat) => set({ previewFormat }),
  chatFocus: 0,
  focusChat: () => set((s) => ({ chatFocus: s.chatFocus + 1 })),
  ...keptSafeArea(),
  setSafeArea: (safeArea) =>
    set((s) => {
      const next = { safeArea, lastSafeArea: safeArea === 'off' ? s.lastSafeArea : safeArea };
      keepSafeArea(next);
      return next;
    }),
  libraryTab: 'clips',
  setLibraryTab: (libraryTab) => set({ libraryTab }),
  autoCopies: keptAutoCopies(),
  setAutoCopies: (autoCopies) => {
    try {
      localStorage.setItem(AUTO_COPIES, autoCopies ? 'on' : 'off');
    } catch {
      // Storage refused: the switch holds for this session.
    }
    set({ autoCopies });
  },
}));
