import { create } from 'zustand';
import type { FormatKind } from '../../bindings/FormatKind';
import type { Keep } from '../../bindings/Keep';
import { speakerBridge } from '../../bridge/speaker';
import { useProject } from '../../state/project';
import { toast } from '../../state/toasts';

/** One clip's follow in one format. */
const runKey = (clipId: string, format: FormatKind) => `${clipId}:${format}`;

interface SpeakerRuns {
  /** How far each follow under way is (0 to 100), by `runKey`. */
  runs: Record<string, number>;
  /** The changes of who is kept under way, by `runKey`: nothing is tracked for them, so no progress is shown. */
  keeping: Record<string, true>;
}

/**
 * The follows under way, so a clip card shows its progress again when the clip is selected again while Rust still
 * tracks. Progress comes from `speaker://progress`, for every clip a follow names.
 */
export const useSpeakerRuns = create<SpeakerRuns>(() => ({ runs: {}, keeping: {} }));

const setRun = (key: string, pct: number | null) =>
  useSpeakerRuns.setState((s) => {
    const runs = { ...s.runs };
    if (pct === null) delete runs[key];
    else runs[key] = pct;
    return { runs };
  });

/** How far clip `clipId`'s follow in `format` is, or null when none is under way. */
export const useSpeakerRun = (clipId: string, format: FormatKind) =>
  useSpeakerRuns((s) => s.runs[runKey(clipId, format)] ?? null);

/** Whether a change of who clip `clipId` keeps in `format` is under way. */
export const useSpeakerKeep = (clipId: string, format: FormatKind) =>
  useSpeakerRuns((s) => s.keeping[runKey(clipId, format)] === true);

const setKeeping = (key: string, on: boolean) =>
  useSpeakerRuns.setState((s) => {
    const keeping = { ...s.keeping };
    if (on) keeping[key] = true;
    else delete keeping[key];
    return { keeping };
  });

/**
 * Follows the speaker in clip `clipId` in `format`: its progress shown while Rust tracks, and its change (one undo step,
 * already in Rust's history) taken into the project once it lands. A failure is said in a toast. Resolves with whether
 * it landed. A second ask while one runs for the same clip and format does nothing.
 */
export async function followSpeaker(clipId: string, format: FormatKind): Promise<boolean> {
  const key = runKey(clipId, format);
  if (useSpeakerRuns.getState().runs[key] !== undefined) return false;
  setRun(key, 0);
  const stop = await speakerBridge
    .onProgress((p) => {
      if (p.clipIds.includes(clipId) && useSpeakerRuns.getState().runs[key] !== undefined) setRun(key, p.pct);
    })
    .catch(() => () => undefined);
  try {
    const followed = await speakerBridge.follow(clipId, format);
    await useProject.getState().adopt(followed.change);
    // Who speaks couldn't be told: say why the crop follows the main person.
    if (followed.note) toast.info(followed.note);
    return true;
  } catch (error) {
    toast.error(error instanceof Error ? error.message : String(error));
    return false;
  } finally {
    stop();
    setRun(key, null);
  }
}

/**
 * Keeps `keep` in frame in clip `clipId` in `format` (Rust makes it from what the follow tracked, at no charge), its
 * change taken into the project as one undo step. A failure is said in a toast. Nothing while a follow of the same
 * clip and format runs.
 */
export async function keepInFrame(clipId: string, format: FormatKind, keep: Keep): Promise<boolean> {
  const key = runKey(clipId, format);
  const { runs, keeping } = useSpeakerRuns.getState();
  if (runs[key] !== undefined || keeping[key]) return false;
  setKeeping(key, true);
  try {
    const change = await speakerBridge.keep(clipId, format, keep);
    await useProject.getState().adopt(change);
    return true;
  } catch (error) {
    toast.error(error instanceof Error ? error.message : String(error));
    return false;
  } finally {
    setKeeping(key, false);
  }
}
