import { create } from 'zustand';
import type { FormatKind } from '../bindings/FormatKind';
import type { Theme } from '../bindings/Theme';
import type { SafeAreaId } from '../lib/safeAreas';

/**
 * The view settings the editor reads: the timeline's zoom, snapping and track view, the format the preview shows, and
 * the theme. Panel widths and heights live in Klipolo's layout (the CSS grid's variables), not here.
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
  /** Show all tracks: every track in a row of its own with its header, instead of the four lanes. */
  timelineExpanded: boolean;
  setTimelineExpanded: (expanded: boolean) => void;
  /** The format the preview shows; null: the project's primary one. */
  previewFormat: FormatKind | null;
  setPreviewFormat: (format: FormatKind | null) => void;
  /** Bumped by Command K: the chat panel opens and puts the cursor in its composer. */
  chatFocus: number;
  focusChat: () => void;
  /** The 9:16 safe area the preview shows ("Show safe area"), or off. */
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
  /** Settings' Appearance: the system's (the default), or always light or always dark. Kept on this Mac. */
  theme: Theme;
  setTheme: (theme: Theme) => void;
}

/** The Library's tabs: your clips, music, sounds and images. */
export type LibraryTab = 'clips' | 'music' | 'sounds' | 'images';

/** Where the Auto copies switch is kept (local storage: `off` when the person turned it off). */
const AUTO_COPIES = 'klipolo.autoCopies';

/** The Auto copies switch as the person left it; on when nothing was kept, or storage is refused. */
function keptAutoCopies(): boolean {
  try {
    return localStorage.getItem(AUTO_COPIES) !== 'off';
  } catch {
    return true;
  }
}

/** Where the theme is kept (local storage), read before the first paint so the page never shows another for a frame. */
const THEME = 'klipolo.theme';
const THEMES: readonly Theme[] = ['system', 'light', 'dark'];

/**
 * The theme as the person left it; the system's when nothing was kept, a value this Klipolo doesn't know, or storage is
 * refused.
 */
function keptTheme(): Theme {
  try {
    const kept = localStorage.getItem(THEME);
    return THEMES.find((theme) => theme === kept) ?? 'system';
  } catch {
    return 'system';
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
  setTimelineExpanded: (timelineExpanded) => set({ timelineExpanded }),
  previewFormat: null,
  setPreviewFormat: (previewFormat) => set({ previewFormat }),
  chatFocus: 0,
  focusChat: () => set((s) => ({ chatFocus: s.chatFocus + 1 })),
  safeArea: 'off',
  lastSafeArea: 'all',
  setSafeArea: (safeArea) =>
    set((s) => ({ safeArea, lastSafeArea: safeArea === 'off' ? s.lastSafeArea : safeArea })),
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
  theme: keptTheme(),
  setTheme: (theme) => {
    try {
      localStorage.setItem(THEME, theme);
    } catch {
      // Storage refused: the theme holds for this session.
    }
    set({ theme });
  },
}));
