import { listen } from '@tauri-apps/api/event';
import { create } from 'zustand';
import type { MissingFont } from '../bindings/MissingFont';
import type { Snapshot } from '../bindings/Snapshot';
import { fonts } from '../bridge/fonts';
import { useProject } from './project';

/**
 * The open project's fonts that aren't on this Mac (Rust's `fonts_missing`; empty until it has answered), and the
 * project opening (`{projectId}:{opening}`) whose banner the person dismissed: once per opening.
 */
interface MissingStore {
  missing: MissingFont[];
  dismissed: string | null;
}

export const useMissingFonts = create<MissingStore>(() => ({ missing: [], dismissed: null }));

/** A burst of edits asks for the missing fonts once, this long after the last. */
const AFTER_EDIT_MS = 500;
/** While Rust says this computer's fonts are still being read (`pending`), it is asked again after this, or sooner on
 * `fonts-scanned`. */
const PENDING_RETRY_MS = 2000;

/** The latest ask: an earlier answer arriving late is dropped. */
let asked = 0;
let timer: ReturnType<typeof setTimeout> | null = null;
let started = false;

function later(ms: number) {
  if (timer) clearTimeout(timer);
  timer = setTimeout(() => {
    timer = null;
    void refreshMissing();
  }, ms);
}

/**
 * Asks Rust which fonts the open project uses that aren't here: the banner's and Replace fonts' one list. Rust answers
 * only once this computer's fonts are read, so an installed font is never called missing at launch; `pending` asks
 * again a little later.
 */
export async function refreshMissing(): Promise<void> {
  const id = useProject.getState().snapshot?.project.id;
  const ask = ++asked;
  if (!id) {
    useMissingFonts.setState({ missing: [] });
    return;
  }
  try {
    const answer = await fonts.missing(id);
    if (ask !== asked || !answer) return;
    if (answer.type === 'pending') later(PENDING_RETRY_MS);
    else if (useProject.getState().snapshot?.project.id === id) useMissingFonts.setState({ missing: answer.fonts });
  } catch {
    // Outside the app (tests, a plain browser), or refused: the list stays as it is.
  }
}

/**
 * Keeps the list current (the banner starts it): asked again after each edit (once a burst of them settles), when
 * another project or opening comes (the old list goes at once), and when fonts arrive or change (`fonts-scanned`,
 * `fonts-changed`: a download or a removal).
 */
export function startMissingFeed() {
  if (started) return;
  started = true;
  useProject.subscribe((now, before) => {
    const a = now.snapshot;
    const b = before.snapshot;
    if (a?.project.id !== b?.project.id || a?.opening !== b?.opening) {
      useMissingFonts.setState({ missing: [] });
      void refreshMissing();
    } else if (a?.history.revision !== b?.history.revision) later(AFTER_EDIT_MS);
  });
  for (const event of ['fonts-scanned', 'fonts-changed'])
    listen(event, () => void refreshMissing()).catch(() => undefined);
}

/** Which opening of which project `snapshot` is: the banner shows once per opening. */
export const openingKey = (snapshot: Snapshot | null | undefined): string | null =>
  snapshot ? `${snapshot.project.id}:${snapshot.opening ?? 0}` : null;

/** The banner is dismissed for this opening of the open project. */
export function dismissMissing() {
  useMissingFonts.setState({ dismissed: openingKey(useProject.getState().snapshot) });
}

/** No list, nothing dismissed, nothing waiting (tests). */
export function resetMissingForTests() {
  asked += 1;
  if (timer) clearTimeout(timer);
  timer = null;
  useMissingFonts.setState({ missing: [], dismissed: null });
}
