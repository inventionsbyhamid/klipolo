import type { Asset } from '../../../bindings/Asset';
import type { Clip } from '../../../bindings/Clip';
import type { ClipSource } from '../../../bindings/ClipSource';
import type { ItemContent } from '../../../bindings/ItemContent';
import type { Recording } from '../../../bindings/Recording';
import type { RecordingSource } from '../../../bindings/RecordingSource';
import { audioSources, firstAsset, mixFor, primarySource, runRate, runSpan } from '../../../lib/recording';
import type { Sources } from '../../../lib/timeline';
import { createLru, type Lru } from './peaksCache';
import type { BlockRuns, FileRun, FileRuns } from './runs';

/** The files a block draws from: its filmstrip's, and its sounds', the voice first (a recording's mic, then its system
 * sound, which draws lighter under it). */
export interface BlockFiles {
  thumbPath?: string;
  /** The filmstrip file's length (µs): how far apart a zoomed-out filmstrip's frames may come from. */
  thumbUs?: number;
  peakPaths: readonly string[];
  /** The sound lead of the file `peakPaths` draws (`Asset.audioLeadUs`, an imported video's). */
  peakLeadUs?: number;
  /**
   * A recording's: every file of its main footage and of each sound source, run by run, where each plays on the
   * recording clock (drift and nudge included, as `runAt`): each thumbnail and waveform comes from the run playing
   * at its time.
   */
  runs?: BlockRuns;
  /** A recording's: the source each of `runs.sounds` is, in the same order (its mix decides how it draws). */
  soundIds?: readonly string[];
}

/** Per project's asset and recording lists (which keep their identity across edits that leave them alone). */
interface Index {
  assets: Map<string, Asset>;
  recordings: Map<string, Recording>;
  /** By source: the same object for every block of it, so `memo`'d blocks see the same props across renders. */
  files: Map<string, BlockFiles | null>;
  /** A recording source's runs, by recording and source (`sourceFiles`). */
  runs: Map<string, FileRuns>;
}

const indexes = new WeakMap<Asset[], WeakMap<Recording[], Index>>();

function indexOf({ assets, recordings }: Sources): Index {
  let byRecordings = indexes.get(assets);
  if (!byRecordings) indexes.set(assets, (byRecordings = new WeakMap()));
  let index = byRecordings.get(recordings);
  if (!index) {
    index = {
      assets: new Map(assets.map((a) => [a.id, a])),
      recordings: new Map(recordings.map((r) => [r.id, r])),
      files: new Map(),
      runs: new Map(),
    };
    byRecordings.set(recordings, index);
  }
  return index;
}

function cached(index: Index, key: string, make: () => BlockFiles | null): BlockFiles | null {
  let files = index.files.get(key);
  if (files === undefined) index.files.set(key, (files = make()));
  return files;
}

const pictures = (asset: Asset | undefined) => (asset?.kind === 'video' ? asset : undefined);
const sounds = (assets: (Asset | undefined)[]) =>
  assets.filter((a) => a !== undefined && a.kind !== 'image' && a.hasAudio).map((a) => a!.path);

function files(film: Asset | undefined, peakPaths: string[]): BlockFiles | null {
  if (film === undefined && peakPaths.length === 0) return null;
  return { thumbPath: film?.path, thumbUs: film?.durationUs, peakPaths };
}

/** Source `s`'s runs whose files are `keep`'s kind (missing files are left out), as `FileRun`s. */
export function sourceRuns(
  assets: Map<string, Asset>,
  s: RecordingSource | undefined,
  keep: (asset: Asset | undefined) => Asset | undefined,
): FileRuns {
  const out: FileRun[] = [];
  for (const run of s?.runs ?? []) {
    const asset = keep(assets.get(run.assetId));
    if (!asset) continue;
    const [fromUs, toUs] = runSpan(run, s!.nudgeUs);
    out.push({
      path: asset.path,
      fromUs,
      toUs,
      rate: runRate(run),
      fileUs: asset.durationUs,
      ...(run.fileInUs && { inUs: run.fileInUs }),
      ...(asset.audioLeadUs && { leadUs: asset.audioLeadUs }),
    });
  }
  return out;
}

/** An imported video whose sound starts apart from its picture: its waveform reads its peaks at file time less
 * the lead (`soundUs`). */
function withLead(shown: BlockFiles | null, asset: Asset | undefined): BlockFiles | null {
  const leadUs = asset?.audioLeadUs ?? 0;
  return shown && leadUs !== 0 ? { ...shown, peakLeadUs: leadUs } : shown;
}

const sound = (asset: Asset | undefined) =>
  asset && asset.kind !== 'image' && asset.hasAudio ? asset : undefined;

/** A main-track clip's: its asset, or its recording's main footage (its screen, else its camera) and its sound
 * sources, the mic first, run by run. */
export function clipFiles(sources: Sources, source: ClipSource): BlockFiles | null {
  const index = indexOf(sources);
  return cached(index, `${source.type}:${source.id}`, () => {
    if (source.type === 'asset') {
      const asset = index.assets.get(source.id);
      return withLead(files(pictures(asset), sounds([asset])), asset);
    }
    const rec = index.recordings.get(source.id);
    if (!rec) return null;
    const asset = (id: string | null) => (id === null ? undefined : index.assets.get(id));
    const voiceFirst = audioSources(rec).sort((a, b) => Number(b.role === 'mic') - Number(a.role === 'mic'));
    const tracks = voiceFirst
      .map((s) => ({ id: s.id, runs: sourceRuns(index.assets, s, sound) }))
      .filter((track) => track.runs.length > 0);
    const runs: BlockRuns = {
      film: sourceRuns(index.assets, primarySource(rec), pictures),
      sounds: tracks.map((track) => track.runs),
    };
    const shown = files(
      pictures(asset(firstAsset(primarySource(rec)))),
      sounds(voiceFirst.map((s) => asset(firstAsset(s)))),
    );
    return shown && { ...shown, runs, soundIds: tracks.map((track) => track.id) };
  });
}

/**
 * A recording's files as each mix of its clips plays them (`clipMediaFiles`), by mix: per recording, since its clips
 * share its files (`clipFiles`) and clips that play the same mix share one object. At most `MIXES_KEPT` mixes a
 * recording, the least recently used let go, so a take whose clips on screen have more distinct mixes than that would
 * make new objects on each render (about 32 small objects a recording; the files themselves are shared).
 */
const mixed = new WeakMap<BlockFiles, Lru<BlockFiles>>();
const MIXES_KEPT = 32;

/**
 * A main-track clip's files (`clipFiles`) as the clip plays them: a recording clip's waveform draws each sound at its
 * volume in the clip's mix (its own setting, else the source's default: `mixFor`) and leaves out a muted one, which
 * includes a sound separated onto its own track (separating mutes it in the clip; its layer draws it). The same object
 * for the same mix, so `memo`'d blocks keep their props. O(sources).
 */
export function clipMediaFiles(sources: Sources, clip: Clip): BlockFiles | null {
  const files = clipFiles(sources, clip.source);
  const rec = clip.source.type === 'recording' ? indexOf(sources).recordings.get(clip.source.id) : undefined;
  if (!files?.runs || !files.soundIds || !rec) return files;
  const mixes = files.soundIds.map((id) => {
    const s = rec.sources.find((x) => x.id === id);
    return s ? mixFor(clip, s) : null;
  });
  const gains = mixes.map((mix) => (!mix || mix.muted ? 0 : mix.volume));
  // A centred sound draws the channel it plays.
  const centres = mixes.map((mix) => mix?.centreFrom ?? null);
  const centred = centres.some((c) => c !== null);
  if (gains.every((gain) => gain === 1) && !centred) return files;
  const key = `${gains.join()}|${centres.join()}`;
  let byMix = mixed.get(files);
  if (!byMix) mixed.set(files, (byMix = createLru(MIXES_KEPT)));
  let out = byMix.get(key);
  if (!out) {
    const heard = gains.flatMap((gain, i) => (gain > 0 ? [i] : []));
    const runs: BlockRuns = {
      film: files.runs.film,
      sounds: heard.map((i) => files.runs!.sounds[i]),
      gains: heard.map((i) => gains[i]),
      ...(centred && { centres: heard.map((i) => centres[i]) }),
    };
    out = { ...files, runs, soundIds: heard.map((i) => files.soundIds![i]) };
    byMix.set(key, out);
  }
  return out;
}

/**
 * Recording `recording`'s sound source `source`, run by run (a source lane's waveforms); the same object every time.
 */
export function sourceFiles(sources: Sources, recording: string, source: string): FileRuns {
  const index = indexOf(sources);
  const key = `source:${recording}:${source}`;
  let runs = index.runs.get(key);
  if (!runs) {
    const s = index.recordings.get(recording)?.sources.find((x) => x.id === source);
    index.runs.set(key, (runs = sourceRuns(index.assets, s, sound)));
  }
  return runs;
}

/** A layer's: a video layer's filmstrip, a sound layer's waveform; other layers draw no media. */
export function itemFiles(sources: Sources, content: ItemContent): BlockFiles | null {
  if (content.type !== 'video' && content.type !== 'audio') return null;
  const index = indexOf(sources);
  return cached(index, `${content.type}:${content.assetId}`, () => {
    const asset = index.assets.get(content.assetId);
    // A video file's sound separated onto a track plays at its lead, as on the clip it came from.
    return content.type === 'video'
      ? files(pictures(asset), [])
      : withLead(files(undefined, sounds([asset])), asset);
  });
}
