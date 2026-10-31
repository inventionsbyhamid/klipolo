import type { Asset } from '../bindings/Asset';
import type { Clip } from '../bindings/Clip';
import type { Project } from '../bindings/Project';

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
