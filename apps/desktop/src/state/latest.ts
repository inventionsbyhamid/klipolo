import type { CameraLayout } from '../bindings/CameraLayout';
import type { Clip } from '../bindings/Clip';
import type { ClipPatch } from '../bindings/ClipPatch';
import type { EffectInstance } from '../bindings/EffectInstance';
import type { EffectTarget } from '../bindings/EffectTarget';
import type { Item } from '../bindings/Item';
import type { ItemContent } from '../bindings/ItemContent';
import type { Layout } from '../bindings/Layout';
import type { Op } from '../bindings/Op';
import type { Settings } from '../bindings/Settings';
import type { Track } from '../bindings/Track';
import type { Label } from '../lib/labels';
import { clipOp, itemOp } from '../lib/ops';
import { commitLatest } from './commit';
import { useProject } from './project';

/*
 * Ops carry whole values (a layer's content or layout, a clip's camera or framing, the settings, a point, an index in
 * a stack), so they are built from the project as it is when they go in, not as a render saw it: a second control used
 * before the first change has landed builds on that change instead of putting the old value back. Every edit enters
 * the commit queue when it is asked for (`commitLatest`, a burst, a slider), and its builder runs at its turn and sets
 * only its own fields over the value the project has then: the forms at the end of this file.
 */

/** One undo step built when it goes in, holding the commit queue until it lands (shared with bursts and sliders). */
export { commitLatest };

/**
 * Each revision's layers and clips by id: built once per array (a patch keeps `tracks` when no layer changed and `main`
 * when no clip did), then O(1). A Centre drag asks every dry-run frame; the old arrays' maps go with them.
 */
const byTracks = new WeakMap<Track[], Map<string, Item>>();
const byMain = new WeakMap<Clip[], Map<string, Clip>>();

/** `array`'s index from `cache`, made from `entries` the first time it is asked for. */
function byId<A extends object, T extends { id: string }>(
  cache: WeakMap<A, Map<string, T>>,
  array: A,
  entries: (array: A) => Iterable<T>,
): Map<string, T> {
  let index = cache.get(array);
  if (!index) {
    index = new Map();
    for (const entry of entries(array)) index.set(entry.id, entry);
    cache.set(array, index);
  }
  return index;
}

function* itemsOf(tracks: Track[]) {
  for (const track of tracks) yield* track.items;
}
const clipsOf = (main: Clip[]) => main;

/** Layer `id` as the project has it now. */
export function itemNow(id: string): Item | null {
  const tracks = useProject.getState().snapshot?.project.timeline.tracks;
  return (tracks && byId(byTracks, tracks, itemsOf).get(id)) ?? null;
}

/** Clip `id` as the project has it now. */
export function clipNow(id: string): Clip | null {
  const main = useProject.getState().snapshot?.project.timeline.main;
  return (main && byId(byMain, main, clipsOf).get(id)) ?? null;
}

/** Layer `id`'s content now, while it is still of `rendered`'s kind; else `rendered`. */
export function contentNow<C extends ItemContent>(id: string, rendered: C): C {
  const now = itemNow(id)?.content;
  return now?.type === rendered.type ? (now as C) : rendered;
}

/** The effect stack `target` names, as the project has it now (null when the clip or layer is gone). */
export function effectsNow(target: EffectTarget): EffectInstance[] | null {
  return (target.type === 'item' ? itemNow(target.id) : clipNow(target.id))?.effects ?? null;
}

/** Layer `id`'s layout now; else `rendered`. */
export function layoutNow(id: string, rendered: Layout): Layout {
  return itemNow(id)?.layout ?? rendered;
}

/** Clip `id`'s camera now (its bubble, modes, shape, border and mirror); else `rendered`. */
export function cameraNow(clipId: string, rendered: CameraLayout): CameraLayout {
  return clipNow(clipId)?.camera ?? rendered;
}

/** The project's settings now (its formats and primary format), or null with no project open. */
export function settingsNow(): Settings | null {
  return useProject.getState().snapshot?.project.settings ?? null;
}

/** What a change sets: its own fields, or a function of the value as the project has it when the change is built. */
export type PatchOf<T> = Partial<T> | ((now: T) => Partial<T>);

/** `now` with `patch` over it. */
function patched<T extends object>(now: T, patch: PatchOf<T>): T {
  return { ...now, ...(typeof patch === 'function' ? (patch as (now: T) => Partial<T>)(now) : patch) };
}

/*
 * Builders: called as the change is built (a burst's or a slider's builder, a `commitLatest` builder, a dry run's
 * frame), never at the press. Each reads the value as the project has it then and sets only `patch` over it (O(1):
 * the per-revision index above).
 */

/** Layer `item`'s content (as `rendered`, while it is still of that kind) with `patch`. */
export function contentOps<C extends ItemContent>(item: Item, rendered: C, patch: PatchOf<C>): Op[] {
  return [itemOp(item, { content: patched(contentNow(item.id, rendered), patch) })];
}

/** Layer `item`'s layout with `patch` (its opacity, its corners, its motion…). */
export function layoutOps(item: Item, patch: PatchOf<Layout>): Op[] {
  return [itemOp(item, { layout: patched(layoutNow(item.id, item.layout), patch) })];
}

/** Clip `clip`'s camera (as `rendered`, while the clip has one) with `patch`. */
export function cameraOps(clip: Clip, rendered: CameraLayout, patch: PatchOf<CameraLayout>): Op[] {
  return [clipOp(clip, { camera: patched(cameraNow(clip.id, rendered), patch) })];
}

/* One-shot changes: one undo step each, entering the commit queue at once (`commitLatest`), built at its turn. */

/** Sets `patch` on layer `item`'s content. */
export const editContent = <C extends ItemContent>(
  label: Label,
  item: Item,
  rendered: C,
  patch: PatchOf<C>,
) => commitLatest(label, () => contentOps(item, rendered, patch));

/** Sets `patch` on layer `item`'s layout. */
export const editLayout = (label: Label, item: Item, patch: PatchOf<Layout>) =>
  commitLatest(label, () => layoutOps(item, patch));

/** Sets clip `clip`'s fields: `patch`, or what it makes of the clip as the project has it then. */
export const editClip = (label: Label, clip: Clip, patch: ClipPatch | ((now: Clip) => ClipPatch)) =>
  commitLatest(label, () => [
    clipOp(clip, typeof patch === 'function' ? patch(clipNow(clip.id) ?? clip) : patch),
  ]);

/**
 * Sets what `patch` makes of the settings as the project has them then (`setSettings` carries them whole); null: the
 * change has nothing to do by then, and nothing goes in.
 */
export const editSettings = (label: Label, patch: (now: Settings) => Partial<Settings> | null) =>
  commitLatest(label, (): Op[] => {
    const now = settingsNow();
    const change = now && patch(now);
    return now && change ? [{ type: 'setSettings', settings: { ...now, ...change } }] : [];
  });
