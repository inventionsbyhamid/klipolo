import type { Asset } from '../bindings/Asset';
import type { Clip } from '../bindings/Clip';
import type { Project } from '../bindings/Project';
import type { SourceRole } from '../bindings/SourceRole';

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

/** What analysis offers in a project, and which recording each voice file is the voice of. */
interface Offer {
  out: Asset[];
  voices: Map<string, string>;
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
  for (const clip of main) {
    if (clip.source.type === 'asset') {
      alone.add(clip.source.id);
      continue;
    }
    const id = clip.source.id;
    const sources = recordings.find((r) => r.id === id)?.sources ?? [];
    const voice = VOICE_ROLES.map((role) =>
      sources.find((s) => s.role === role && s.runs.some((run) => sounding.has(run.assetId))),
    ).find((source) => source !== undefined);
    for (const run of voice?.runs ?? []) if (sounding.has(run.assetId)) voices.set(run.assetId, id);
  }
  const out = analysedAssets(project).filter((a) => alone.has(a.id) || voices.has(a.id));
  const offer = { out, voices };
  offers.set(main, { assets, recordings, offer });
  return offer;
}

/**
 * The files analysis offers, in the project's order: a recording counts as one clip. Of the files main-track clips
 * play (`analysedAssets`), each one a clip plays on its own, and each recording's voice: the files of its first source
 * with a file that has sound, in the order mic, camera, screen, as the model's `speech::voice_runs` picks it (its
 * captions read their words). A recording's other files (its screen and camera beside a mic) are never offered, so
 * they are never sent. The app analyses any file of a recording for its words only. The same array for the same main
 * track, assets and recordings, so a store selector can return it.
 */
export function analysable(project: AnalysedScope): Asset[] {
  return offerOf(project).out;
}
