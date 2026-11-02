import type { Asset } from '../bindings/Asset';
import type { Clip } from '../bindings/Clip';
import type { Project } from '../bindings/Project';
import type { Recording } from '../bindings/Recording';
import type { RecordingSource } from '../bindings/RecordingSource';
import type { SourceRole } from '../bindings/SourceRole';
import type { SourceRun } from '../bindings/SourceRun';
import { audioSources, isVideoRole, mixFor, primarySource, runSpan } from './recording';

/** What `analysedAssets` reads of a project. */
export type AnalysedScope = Pick<Project, 'assets' | 'recordings'> & { timeline: Pick<Project['timeline'], 'main'> };

const NO_ASSETS: Asset[] = [];
const NO_RECORDINGS: Project['recordings'] = [];
const NO_CLIPS: Clip[] = [];

/** The last answer for each main track, while its assets and recordings stay the same arrays (a patch keeps them). */
const cache = new WeakMap<Clip[], { assets: Asset[]; recordings: Project['recordings']; out: Asset[] }>();

/**
 * The files analysis is for, in the project's order: every video or sound a main-track clip plays (an imported clip's
 * own file, and each file of a recording's sources). Music, sound effects and pictures go on layers, so a song, a
 * whoosh or a file imported and never placed is neither offered nor paid for. Mirrors the model's
 * `analysed::analysed_assets` (pinned by `analysed-parity.json`), which the app's analysis commands ask. The same array
 * for the same main track, assets and recordings, so a store selector can return it.
 */
export function analysedAssets(project: AnalysedScope): Asset[] {
  // A part a project doesn't have counts as empty.
  const assets = project.assets ?? NO_ASSETS;
  const recordings = project.recordings ?? NO_RECORDINGS;
  const main = project.timeline?.main ?? NO_CLIPS;
  const known = cache.get(main);
  if (known && known.assets === assets && known.recordings === recordings) return known.out;
  const played = new Set<string>();
  for (const clip of main) {
    if (clip.source.type === 'asset') played.add(clip.source.id);
    else {
      const id = clip.source.id;
      const recording = recordings.find((r) => r.id === id);
      for (const source of recording?.sources ?? []) for (const run of source.runs) played.add(run.assetId);
    }
  }
  const out = assets.filter((a) => (a.kind === 'video' || a.kind === 'audio') && played.has(a.id));
  cache.set(main, { assets, recordings, out });
  return out;
}

/** Whose sound carries a recording's voice, best first: the order the model's `speech::voice_runs` picks it in. */
const VOICE_ROLES: SourceRole[] = ['mic', 'camera', 'screen', 'game', 'gameAudio', 'systemAudio'];

/** A gap shorter than this is where two runs meet, not a stretch nothing recorded (`Recording::true_gaps`). */
const MIN_GAP_US = 1_000;

/**
 * The recording-time stretches of `[from, to)` where none of `r`'s sound sources has a run, each at least `MIN_GAP_US`
 * long, as `Recording::true_gaps` finds them: a picture source's own sound may fill them.
 */
function trueGaps(r: Recording, from: number, to: number): [number, number][] {
  let gaps: [number, number][] = from < to ? [[from, to]] : [];
  for (const s of audioSources(r)) {
    for (const run of s.runs) {
      const [a, b] = runSpan(run, s.nudgeUs);
      if (a >= b) continue;
      gaps = gaps.flatMap(([x, y]): [number, number][] => {
        if (y <= a || b <= x) return [[x, y]];
        const kept: [number, number][] = [];
        if (x < a) kept.push([x, a]);
        if (b < y) kept.push([b, y]);
        return kept;
      });
    }
  }
  return gaps.filter(([x, y]) => y - x >= MIN_GAP_US);
}

/** Whether `run` of `s` plays some of the recording-time stretch `[from, to)`. */
function playsIn(s: RecordingSource, run: SourceRun, [from, to]: [number, number]): boolean {
  const [a, b] = runSpan(run, s.nudgeUs);
  return Math.max(a, from) < Math.min(b, to);
}

/**
 * The files whose words a recording clip's captions read, as the model's `speech::voice_runs` picks them: the sounding
 * runs it plays of its recording's first source with a sounding file in `VOICE_ROLES` order, and over the recording's
 * true gaps, those of the first picture source whose own sound plays there (unmuted, or muted to fill them).
 */
function voiceFiles(clip: Clip, r: Recording, sounding: Set<string>): string[] {
  const span: [number, number] = [clip.sourceInUs, clip.sourceOutUs];
  const heard = (s: RecordingSource) => s.runs.some((run) => sounding.has(run.assetId));
  const voice = VOICE_ROLES.map((role) => r.sources.find((s) => s.role === role && heard(s))).find(Boolean);
  if (!voice) return [];
  const files = voice.runs.filter((run) => sounding.has(run.assetId) && playsIn(voice, run, span));
  const filling = r.sources.find((s) => {
    const mix = mixFor(clip, s);
    return isVideoRole(s.role) && mix.volume > 0 && (!mix.muted || !!mix.fillsGaps) && heard(s);
  });
  if (filling && !filling.runs.some((run) => files.some((f) => f.assetId === run.assetId))) {
    for (const gap of trueGaps(r, ...span)) {
      files.push(...filling.runs.filter((run) => sounding.has(run.assetId) && playsIn(filling, run, gap)));
    }
  }
  return files.map((run) => run.assetId);
}

/** A camera take's footage files a clip of it plays (`cameraFootage`); none for any other take. */
function footageFiles(clip: Clip, r: Recording): string[] {
  const footage = primarySource(r);
  if (footage?.role !== 'camera') return [];
  const span: [number, number] = [clip.sourceInUs, clip.sourceOutUs];
  return footage.runs.filter((run) => playsIn(footage, run, span)).map((run) => run.assetId);
}

/**
 * What analysis offers in a project, which recording each voice file is the voice of, and which take each offered file
 * of a take is (its voice, and a camera take's footage).
 */
interface Offer {
  out: Asset[];
  voices: Map<string, string>;
  takes: Map<string, string>;
}

/** The last offer for each main track, while its assets and recordings stay the same arrays. */
const offers = new WeakMap<Clip[], { assets: Asset[]; recordings: Project['recordings']; offer: Offer }>();

function offerOf(project: AnalysedScope): Offer {
  const assets = project.assets ?? NO_ASSETS;
  const recordings = project.recordings ?? NO_RECORDINGS;
  const main = project.timeline?.main ?? NO_CLIPS;
  const known = offers.get(main);
  if (known && known.assets === assets && known.recordings === recordings) return known.offer;
  const sounding = new Set(assets.filter((a) => a.hasAudio).map((a) => a.id));
  const alone = new Set<string>();
  const voices = new Map<string, string>();
  const footage = new Map<string, string>();
  for (const clip of main) {
    if (clip.source.type === 'asset') {
      // A take's file is heard for its words only, so one with no sound has nothing to analyse.
      const id = clip.source.id;
      if (sounding.has(id) || recordingOf(project, id) === null) alone.add(id);
      continue;
    }
    const id = clip.source.id;
    const recording = recordings.find((r) => r.id === id);
    if (recording) for (const file of voiceFiles(clip, recording, sounding)) voices.set(file, id);
    // A camera take's camera is its footage: watched for a scene log, as imported camera footage is.
    if (recording) for (const file of footageFiles(clip, recording)) footage.set(file, id);
  }
  const out = analysedAssets(project).filter((a) => alone.has(a.id) || voices.has(a.id) || footage.has(a.id));
  const takes = new Map([...voices, ...[...footage].filter(([file]) => !voices.has(file))]);
  const offer = { out, voices, takes };
  offers.set(main, { assets, recordings, offer });
  return offer;
}

/**
 * The files analysis offers, in the project's order: a recording counts as one clip. Of the files main-track clips
 * play (`analysedAssets`), each one a clip plays on its own (a take's file only when it has sound), and each
 * recording's voice: the files of its first source with a file that has sound, in the order mic, camera, screen, as
 * the model's `speech::voice_runs` picks it (its captions read their words), with a picture source's own sound where
 * it fills the take's gaps. A camera take's camera is its footage, offered too to be watched for a scene log
 * (`cameraFootage`). A recording's other files (its screen and camera bubble beside a mic) are never offered, so they
 * are never sent. The app analyses any other file of a recording for its words only (`wordsOnly`). The same array for
 * the same main track, assets and recordings, so a store selector can return it.
 */
export function analysable(project: AnalysedScope): Asset[] {
  return offerOf(project).out;
}

/** The recording whose voice `assetId` is (`analysable`), or null: its analysis is the take's, so it reads as one. */
export function voiceOf(project: AnalysedScope, assetId: string): string | null {
  return offerOf(project).voices.get(assetId) ?? null;
}

/**
 * Each offered file of the project's takes, by its recording: the voice files (`voiceOf`), then a camera take's
 * footage, watched beside its voice. A take is one clip: offers, rows and notes count its recording once, and its row
 * is named as its clip. The same map while the main track, assets and recordings stay the same.
 */
export function takes(project: AnalysedScope): ReadonlyMap<string, string> {
  return offerOf(project).takes;
}

/** The recording whose take `assetId` is analysed for (`takes`), or null for a clip of its own. */
export function takeOf(project: AnalysedScope, assetId: string): string | null {
  return offerOf(project).takes.get(assetId) ?? null;
}

/** Whether `ids` hold `assetId`, or a file at the same path. */
function sameFile(assets: Asset[], assetId: string): (id: string) => boolean {
  const path = assets.find((a) => a.id === assetId)?.path;
  return (id) => id === assetId || (path !== undefined && assets.find((a) => a.id === id)?.path === path);
}

/**
 * The recording whose files hold `assetId` (its screen, camera or a sound), or the same file (by its path) imported
 * again; null for a file imported on its own.
 */
export function recordingOf(project: Pick<AnalysedScope, 'assets' | 'recordings'>, assetId: string): string | null {
  const same = sameFile(project.assets ?? NO_ASSETS, assetId);
  const recordings = project.recordings ?? NO_RECORDINGS;
  const holder = recordings.find((r) => r.sources.some((s) => s.runs.some((run) => same(run.assetId))));
  return holder?.id ?? null;
}

/**
 * Whether `assetId` is a camera take's footage: a run of the camera that is a recording's main picture ("Record
 * yourself"), or the same file imported again. The app watches it for a scene log as it does imported camera footage
 * (the model's `analysed::camera_footage_file`).
 */
export function cameraFootage(project: Pick<AnalysedScope, 'assets' | 'recordings'>, assetId: string): boolean {
  const same = sameFile(project.assets ?? NO_ASSETS, assetId);
  return (project.recordings ?? NO_RECORDINGS).some((r) => {
    const footage = primarySource(r);
    return footage?.role === 'camera' && footage.runs.some((run) => same(run.assetId));
  });
}

/**
 * Whether the app analyses `assetId` for its words only, never watching it for a scene log: a recording's file
 * (`recordingOf`) that isn't a camera take's footage (`cameraFootage`). Mirrors the model's `analysed::words_only`,
 * pinned by `voices-parity.json`.
 */
export function wordsOnly(project: Pick<AnalysedScope, 'assets' | 'recordings'>, assetId: string): boolean {
  return recordingOf(project, assetId) !== null && !cameraFootage(project, assetId);
}
