import type { Clip } from '../bindings/Clip';
import { builtinId } from './contrib';
import { recordingDurationUs } from './recording';
import { clipDurationUs, isFreeze, playedUs, primaryAsset, type Sources } from './timeline';
import { US_PER_SEC } from './time';

/** Mirrors `klipolo_model::transitions` (pinned by transition-parity.json). */
export const MIN_TRANSITION_US = 100_000;
export const MAX_TRANSITION_US = 5_000_000;
export const DEFAULT_TRANSITION_US = 500_000;
/** The longest time the model stores (`time::MAX_TIME_US`): 24 hours. */
const MAX_TIME_US = 86_400_000_000;

/** What a new transition is (⌘T, a cut's Add transition): every editor's default. */
export const TRANSITION_DISSOLVE = builtinId('transition.dissolve');

/** What the transition math reads of a project: its clips and the files they play (a `Project` is one). */
export type Footage = Sources & { timeline: { main: Clip[] } };

export function splitDuration(d: number): [number, number] {
  const before = Math.floor(d / 2);
  return [before, d - before];
}

/** How long a clip's source is (a file, or a recording's take: `source_duration_us`): how far its edges can be
 * trimmed out. A freeze frame's range is only its length: it may run as long as the model stores. */
export function sourceLength(project: Sources, clip: Clip): number {
  if (isFreeze(clip)) return MAX_TIME_US;
  const source = clip.source;
  if (source.type === 'asset') return primaryAsset(project, source)?.durationUs ?? clip.sourceOutUs;
  const recording = project.recordings.find((r) => r.id === source.id);
  return recording ? recordingDurationUs(recording) : clip.sourceOutUs;
}

/** The footage, played at the clip's speed, before clip `index`'s in point and after its out point. */
export function handles(project: Footage, index: number): { before: number; after: number } {
  const clip = project.timeline.main[index];
  // A freeze frame holds its frame however long a transition needs.
  if (isFreeze(clip)) return { before: MAX_TRANSITION_US, after: MAX_TRANSITION_US };
  return {
    before: playedUs(Math.max(0, clip.sourceInUs), clip.speed),
    after: playedUs(Math.max(0, sourceLength(project, clip) - clip.sourceOutUs), clip.speed),
  };
}

function fitUs(project: Footage, i: number, incomingAfter: number, nextBefore: number): number {
  const main = project.timeline.main;
  if (i + 1 >= main.length) return 0;
  const roomBefore = Math.min(handles(project, i + 1).before, clipDurationUs(main[i]) - incomingAfter);
  const roomAfter = Math.min(handles(project, i).after, clipDurationUs(main[i + 1]) - nextBefore);
  if (roomBefore <= 0 || roomAfter <= 0) return 0;
  return Math.min(2 * roomBefore + 1, 2 * roomAfter, MAX_TRANSITION_US);
}

/** The longest transition clip `index` can have into the next one, leaving its neighbours' transitions alone. */
export function maxTransitionUs(project: Footage, index: number): number {
  const main = project.timeline.main;
  if (index + 1 >= main.length) return 0;
  const prev = index > 0 ? main[index - 1].transitionOut : undefined;
  const next = main[index + 1].transitionOut;
  return fitUs(
    project,
    index,
    prev ? splitDuration(prev.durationUs)[1] : 0,
    next ? splitDuration(next.durationUs)[0] : 0,
  );
}

/**
 * Each clip's transition length once an edit's batch is applied (0: none, or dropped as too short), as
 * `klipolo_model::transitions::normalize` refits them: within their footage and clips, earlier cuts first.
 */
export function fittedTransitionsUs(project: Footage): number[] {
  let incomingAfter = 0;
  return project.timeline.main.map((clip, i) => {
    const want = clip.transitionOut?.durationUs;
    const d = want === undefined ? 0 : Math.min(want, fitUs(project, i, incomingAfter, 0));
    if (d < MIN_TRANSITION_US) {
      incomingAfter = 0;
      return 0;
    }
    incomingAfter = splitDuration(d)[1];
    return d;
  });
}

/** How long `frames` frames at `fps` last, to the µs (the engine's frame starts). */
const framesUs = (frames: number, fps: number) => Math.round((frames * US_PER_SEC) / fps);

/**
 * The whole numbers of frames a transition at `fps` can last where it may be up to `maxUs`: [first, last], from the
 * first at least `MIN_TRANSITION_US` long. Empty (first > last) when none fits.
 */
function frameRange(maxUs: number, fps: number): [number, number] {
  let first = Math.ceil((MIN_TRANSITION_US * fps) / US_PER_SEC);
  while (framesUs(first, fps) < MIN_TRANSITION_US) first++;
  let last = Math.floor((maxUs * fps) / US_PER_SEC);
  // A frame count's length is rounded to the µs, so it can land a µs past the limit.
  while (last > 0 && framesUs(last, fps) > maxUs) last--;
  return [first, last];
}

/** Half a second on the frame grid, or the longest whole number of frames that fits; 0 when none does. */
export function defaultTransitionUs(project: Footage, index: number, fps: number): number {
  const [first, last] = frameRange(maxTransitionUs(project, index), fps);
  const frames = Math.min(last, Math.floor((DEFAULT_TRANSITION_US * fps) / US_PER_SEC));
  return frames >= first ? framesUs(frames, fps) : 0;
}

/**
 * `us` on the frame grid, within what a transition can last where it may be up to `maxUs` (a duration slider's value
 * as the model takes it: never under 0.1 s, never past the footage). 0 when no length fits.
 */
export function transitionLengthUs(us: number, maxUs: number, fps: number): number {
  const [first, last] = frameRange(maxUs, fps);
  if (first > last) return 0;
  return framesUs(Math.min(last, Math.max(first, Math.round((us * fps) / US_PER_SEC))), fps);
}

export interface TransitionWindow {
  clipIndex: number;
  startUs: number;
  cutUs: number;
  endUs: number;
}

export function transitionWindows(main: Clip[], starts: ArrayLike<number>): TransitionWindow[] {
  const out: TransitionWindow[] = [];
  for (let i = 0; i + 1 < main.length; i++) {
    const t = main[i].transitionOut;
    if (!t) continue;
    const cutUs = starts[i] + clipDurationUs(main[i]);
    const [before, after] = splitDuration(t.durationUs);
    out.push({ clipIndex: i, startUs: cutUs - before, cutUs, endUs: cutUs + after });
  }
  return out;
}
