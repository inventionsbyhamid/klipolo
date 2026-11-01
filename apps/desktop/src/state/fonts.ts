import { listen } from '@tauri-apps/api/event';
import { create } from 'zustand';
import { createJSONStorage, persist } from 'zustand/middleware';
import type { DownloadedId } from '../bindings/DownloadedId';
import type { FontRef } from '../bindings/FontRef';
import { fonts } from '../bridge/fonts';
import { forgetFamily, forgetPreview } from '../lib/fonts';

/** How far a Google download has come (`fonts-download`). */
export interface DownloadProgress {
  done: number;
  total: number;
}

/**
 * What the page knows of Google Fonts: Settings' switch and the person's one consent (Rust refuses downloads and
 * previews without them whatever this says), whether those are Rust's yet, and the downloads running now.
 */
interface FontsStore {
  /** Settings' "Google Fonts" switch: off, the menu lists no family that needs a download. */
  googleFonts: boolean;
  /** "Download from Google Fonts?" was confirmed once: downloads go at once, and online previews load. */
  googleConsented: boolean;
  /** Whether the two above are Rust's yet (until then a pick asks Rust first, so a consent given is never asked again). */
  googleLoaded: boolean;
  /** The Google downloads running now, by family. */
  downloads: Record<string, DownloadProgress>;
}

export const useFonts = create<FontsStore>(() => ({
  googleFonts: true,
  googleConsented: false,
  googleLoaded: false,
  downloads: {},
}));

/** Google Fonts' switch and consent as Rust has them (the font menu asks as it opens). */
export async function loadGoogleFonts(): Promise<void> {
  try {
    const g = await fonts.google();
    if (g) useFonts.setState({ googleFonts: g.enabled, googleConsented: g.consented, googleLoaded: true });
  } catch {
    // Outside the app it stays as it is.
  }
}

/** The person's one consent, then the setting as Rust saved it. */
export async function consentToGoogleFonts(): Promise<void> {
  const g = await fonts.consent();
  useFonts.setState({ googleFonts: g.enabled, googleConsented: g.consented, googleLoaded: true });
}

/** A download running now: one request, which every pick of that face waits on. */
interface Held {
  done: Promise<FontRef>;
  waiters: number;
  family: string;
}

/** The downloads running now, by face (a family fetched a face at a time never joins another weight's). */
const running = new Map<string, Held>();
/** Each family's downloads running now: its one progress entry and Cancel last while any of them does. */
const families = new Map<string, Set<Held>>();

const faceKey = (family: string, weight: number, italic: boolean) => `${family}|${weight}|${italic ? 'i' : 'n'}`;

function dropProgress(family: string) {
  useFonts.setState((s) => {
    const downloads = { ...s.downloads };
    delete downloads[family];
    return { downloads };
  });
}

/**
 * Downloads a Google family for a text at `weight` and `italic` (its row shows the progress meanwhile), then its
 * `FontRef`: the fonts' change reaches every face through `fonts-changed`. The same face already downloading is
 * joined, never fetched twice; one Cancel stops it for every pick waiting on it. A refusal or failure comes as Rust's
 * code.
 */
export async function downloadFont(
  family: string,
  version: string | null,
  weight: number,
  italic: boolean,
): Promise<FontRef> {
  const key = faceKey(family, weight, italic);
  let held = running.get(key);
  if (!held) {
    held = { done: fonts.download(family, version, weight, italic), waiters: 0, family };
    running.set(key, held);
    let mine = families.get(family);
    if (!mine) {
      mine = new Set();
      families.set(family, mine);
      useFonts.setState((s) => ({ downloads: { ...s.downloads, [family]: { done: 0, total: 0 } } }));
    }
    mine.add(held);
  }
  held.waiters += 1;
  const own = held;
  try {
    return await own.done;
  } finally {
    own.waiters -= 1;
    if (own.waiters === 0) {
      if (running.get(key) === own) running.delete(key);
      // A Cancel may have let it go already, and a new pick started the family afresh: that one stays.
      const mine = families.get(family);
      if (mine?.delete(own) && mine.size === 0) {
        families.delete(family);
        dropProgress(family);
      }
    }
  }
}

/**
 * Stops `family`'s downloads, for every pick waiting on them (their finished files stay). They are let go at once: a
 * pick made while they end asks Rust afresh, never ends as this cancel.
 */
export function cancelDownload(family: string): void {
  forgetDownloads(family);
  void fonts.cancel(family).catch(() => undefined);
}

/** `family`'s downloads let go: the picks waiting on them still hear how they end; a new pick starts afresh. */
function forgetDownloads(family: string): void {
  for (const [key, held] of running) if (held.family === family) running.delete(key);
  if (families.delete(family)) dropProgress(family);
}

/**
 * Settings' switch, as Rust saved it. Off stops the downloads running now, so they are let go here too: a pick once it
 * is on again asks Rust afresh, never ends as that cancel.
 */
export async function setGoogleFonts(enabled: boolean): Promise<void> {
  if (!enabled) for (const family of [...families.keys()]) forgetDownloads(family);
  const g = await fonts.setGoogle(enabled);
  useFonts.setState({ googleFonts: g.enabled, googleConsented: g.consented, googleLoaded: true });
}

/** Settings' Remove of a downloaded family (Rust stops its download first): let go here too, as a Cancel is. */
export function removeDownloaded(id: DownloadedId): Promise<void> {
  forgetDownloads(id.family);
  return fonts.remove(id);
}

/** No download is running (tests: one a test left waiting is never joined by the next). */
export function resetDownloadsForTests() {
  running.clear();
  families.clear();
  useFonts.setState({ downloads: {} });
}

/** A family's fonts changed (a download): its faces are asked again, and its preview's let go of. */
export function fontsChanged(family: string) {
  forgetFamily(family);
  forgetPreview(family);
}

let started = false;

/**
 * Keeps the page in step with Rust's fonts: a family's fonts changing (`fonts-changed`, a download) makes its faces be
 * asked again, and `fonts-download` is a download's progress. Started by the first font menu.
 */
export function startFontsFeed() {
  if (started) return;
  started = true;
  listen<{ family: string }>('fonts-changed', (e) => fontsChanged(e.payload.family)).catch(() => undefined);
  listen<{ family: string; done: number; total: number }>('fonts-download', (e) => {
    const { family, done, total } = e.payload;
    if (useFonts.getState().downloads[family])
      useFonts.setState((s) => ({ downloads: { ...s.downloads, [family]: { done, total } } }));
  }).catch(() => undefined);
}

/** How many recently picked fonts the font menu lists ("Recent"). */
export const RECENT_FONTS_MAX = 8;

/** Whether `a` and `b` name the same font: the same family from the same source. */
export function sameFont(a: FontRef, b: FontRef): boolean {
  return a.family === b.family && JSON.stringify(a.source) === JSON.stringify(b.source);
}

/** A saved font as the store keeps it, or null when it isn't one. */
function savedFont(value: unknown): FontRef | null {
  if (typeof value !== 'object' || value === null) return null;
  const { family, source } = value as { family?: unknown; source?: { type?: unknown; version?: unknown } };
  if (typeof family !== 'string' || typeof source !== 'object' || source === null) return null;
  if (source.type === 'bundled' || source.type === 'installed') return { family, source: { type: source.type } };
  if (source.type === 'google' && typeof source.version === 'string')
    return { family, source: { type: 'google', version: source.version } };
  return null;
}

interface RecentFonts {
  /** The fonts picked last, newest first, each once, at most `RECENT_FONTS_MAX`. */
  recent: FontRef[];
  push(font: FontRef): void;
}

/**
 * The person's last font picks, for the font menu's "Recent", the same in every project and kept across launches
 * (`klipolo-fonts`). A saved value of the wrong shape reads as none, never a crash.
 */
export const useRecentFonts = create<RecentFonts>()(
  persist(
    (set, get) => ({
      recent: [],
      push: (font) =>
        set({ recent: [font, ...get().recent.filter((f) => !sameFont(f, font))].slice(0, RECENT_FONTS_MAX) }),
    }),
    {
      name: 'klipolo-fonts',
      storage: createJSONStorage(() => localStorage),
      partialize: ({ recent }) => ({ recent }),
      merge: (saved, current) => {
        const list = typeof saved === 'object' && saved !== null ? (saved as { recent?: unknown }).recent : undefined;
        const recent = Array.isArray(list) ? list.map(savedFont).filter((f): f is FontRef => f !== null) : [];
        return { ...current, recent: recent.slice(0, RECENT_FONTS_MAX) };
      },
    },
  ),
);
