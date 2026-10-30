import type { Project } from '../bindings/Project';
import type { ProjectPatch } from '../bindings/ProjectPatch';
import type { Track } from '../bindings/Track';
import type { TrackShape } from '../bindings/TrackShape';

/** Every patch field outside the timeline. */
type TopLevelKey = Exclude<keyof ProjectPatch, 'main' | 'clips' | 'tracks' | 'items'>;

/**
 * Checked against `ProjectPatch` both ways: a field Rust adds to the patch fails `tsc` here (missing key) instead of
 * being dropped, and one it removes fails as an excess key. `set` below checks each is a `Project` field too.
 */
const TOP_LEVEL_FIELDS = {
  name: true,
  settings: true,
  assets: true,
  recordings: true,
  luts: true,
  captions: true,
  background: true,
  audio: true,
  cursor: true,
  wordFixes: true,
  wordLooks: true,
  requires: true,
} satisfies Record<TopLevelKey, true>;
const TOP_LEVEL = Object.keys(TOP_LEVEL_FIELDS) as TopLevelKey[];

/** A track's own fields (all but its items), checked against `TrackShape` both ways like `TOP_LEVEL_FIELDS`. */
const TRACK_FIELDS = {
  id: true,
  kind: true,
  name: true,
  hidden: true,
  muted: true,
  locked: true,
  solo: true,
} satisfies Record<Exclude<keyof TrackShape, 'items'>, true>;
const TRACK_KEYS = Object.keys(TRACK_FIELDS) as (keyof typeof TRACK_FIELDS)[];

/** Whether `track` has `shape`'s settings. An optional flag Rust leaves out (`solo` off) counts as false. */
function sameSettings(track: Track, shape: TrackShape): boolean {
  return TRACK_KEYS.every((key) => (track[key] ?? false) === (shape[key] ?? false));
}

function sameArray<T>(a: readonly T[], b: readonly T[]): boolean {
  return a.length === b.length && a.every((x, i) => x === b[i]);
}

/**
 * Looks `ids` up in the patch's changed objects, then the mirror's. Null when an id is in neither, or a changed object
 * isn't placed: the mirror and Rust disagree, and the caller fetches the whole project rather than drop something.
 */
function resolve<T>(
  ids: readonly string[],
  changed: Map<string, T>,
  old: Map<string, T>,
  used: { n: number },
) {
  const out: T[] = [];
  for (const id of ids) {
    const fresh = changed.get(id);
    if (fresh !== undefined) used.n++;
    const value = fresh ?? old.get(id);
    if (value === undefined) return null;
    out.push(value);
  }
  return out;
}

/**
 * `project` with `patch` applied, reusing every object the patch leaves alone: unchanged clips, items and
 * tracks, and the arrays that hold them, stay the very same objects, so `memo`'d timeline blocks don't re-render.
 * A track whose settings alone change keeps its `items` array. Null when the patch names a clip or item the mirror
 * doesn't have (the caller resyncs through `project_snapshot`).
 */
export function applyPatch(project: Project, patch: ProjectPatch): Project | null {
  let next: Project = project;
  const set = <K extends keyof Project>(key: K, value: Project[K]) => {
    if (next === project) next = { ...project };
    next[key] = value;
  };
  for (const key of TOP_LEVEL) {
    const value = patch[key];
    if (value !== undefined) set(key, value as Project[typeof key]);
  }
  const clipsChanged = patch.main !== undefined || patch.clips.length > 0;
  const itemsChanged = patch.tracks !== undefined || patch.items.length > 0;
  if (!clipsChanged && !itemsChanged) return next;

  const { timeline } = project;
  let main = timeline.main;
  if (clipsChanged) {
    const changed = new Map(patch.clips.map((c) => [c.id, c]));
    const used = { n: 0 };
    const order = patch.main ?? main.map((c) => c.id);
    const clips = resolve(order, changed, new Map(main.map((c) => [c.id, c])), used);
    if (!clips || used.n !== changed.size) return null;
    if (!sameArray(clips, main)) main = clips;
  }
  let tracks = timeline.tracks;
  if (itemsChanged) {
    const changed = new Map(patch.items.map((i) => [i.id, i]));
    const used = { n: 0 };
    const old = new Map(tracks.flatMap((track) => track.items.map((item) => [item.id, item] as const)));
    const oldTracks = new Map(tracks.map((t) => [t.id, t]));
    const shapes =
      patch.tracks ?? tracks.map((t) => ({ ...t, items: t.items.map((i) => i.id) }) satisfies TrackShape);
    const rebuilt: Track[] = [];
    for (const shape of shapes) {
      const before = oldTracks.get(shape.id);
      const resolved = resolve(shape.items, changed, old, used);
      if (!resolved) return null;
      const items = before && sameArray(resolved, before.items) ? before.items : resolved;
      if (before && items === before.items && sameSettings(before, shape)) rebuilt.push(before);
      // Every setting the shape carries (`solo` too), and the items it names.
      else rebuilt.push({ ...shape, items } satisfies Track);
    }
    if (used.n !== changed.size) return null;
    if (!sameArray(rebuilt, tracks)) tracks = rebuilt;
  }
  if (main !== timeline.main || tracks !== timeline.tracks) set('timeline', { main, tracks });
  return next;
}
