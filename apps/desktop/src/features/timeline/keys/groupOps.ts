import type { Clip } from '../../../bindings/Clip';
import type { EffectInstance } from '../../../bindings/EffectInstance';
import type { Item } from '../../../bindings/Item';
import type { Op } from '../../../bindings/Op';
import type { Project } from '../../../bindings/Project';
import type { Track } from '../../../bindings/Track';
import type { TrackTarget } from '../../../bindings/TrackTarget';
import { newId } from '../../../lib/defaults';
import { snapToFrame } from '../../../lib/time';
import { clipDurationUs, clipHasSound, findItem, hasOwnSound, itemEndUs } from '../../../lib/timeline';
import type { GroupEntry } from '../../../state/clipboard';
import { clipIndexAt, mainIndex } from '../model/index';
import { pasteEffectsOps } from './clipboardOps';

/*
 * What a selected group's edits are, as ops (one undo step each): delete, move and nudge, copy, paste and duplicate,
 * mute and volume, paste effects. Pure, so the keys, the group's card and its drag build the same ones.
 */

/** A group's blocks: its main-track clips (track order) and its layers (with their tracks), from the project now. */
export interface Members {
  clips: { clip: Clip; index: number }[];
  layers: { item: Item; track: Track }[];
}

/** The blocks `ids` names that are still in the project. */
export function membersOf(project: Project, ids: readonly string[]): Members {
  const wanted = new Set(ids);
  const clips = project.timeline.main
    .map((clip, index) => ({ clip, index }))
    .filter(({ clip }) => wanted.has(clip.id));
  const layers: Members['layers'] = [];
  for (const track of project.timeline.tracks)
    for (const item of track.items) if (wanted.has(item.id)) layers.push({ item, track });
  return { clips, layers };
}

/** How many blocks `members` holds. */
export const countOf = (m: Members) => m.clips.length + m.layers.length;

/**
 * Delete: the layers go (those on locked tracks stay), then the clips' time leaves the main track in one
 * `deleteRanges`: gaps close by the ripple rules and free layers follow the cut rules, as a range's delete.
 */
export function groupDeleteOps(project: Project, m: Members): Op[] {
  const ops: Op[] = m.layers
    .filter(({ track }) => !track.locked)
    .map(({ item }) => ({ type: 'removeItem', id: item.id }));
  if (m.clips.length > 0) {
    const index = mainIndex(project.timeline.main);
    const ranges = m.clips.map(({ index: i }) => ({ start: index.starts[i], end: index.ends[i] }));
    ops.push({ type: 'deleteRanges', ranges });
  }
  return ops;
}

/** How many blocks a group's delete takes out (layers on locked tracks stay). */
export const deletedCount = (m: Members) => m.clips.length + m.layers.filter(({ track }) => !track.locked).length;

/** Why a group's layers can't move: one is on a locked track, or a row it would go to can't take it. */
export type MoveRefusal = 'locked' | 'kind' | 'start';

/**
 * Its layers `dtUs` later (earlier when negative) and `rows` tracks up (down when negative), offsets kept: each layer
 * goes to the track `rows` away in the project's order. Refused when one is on a locked track, when a track it would
 * go to is missing, locked or of another kind, or when one would start before 0:00. Across tracks, the layers on the
 * top track go first when going up and those on the bottom one first when going down, so each lands on a track the
 * group's layers there have already left (the model bumps a layer that lands on another to a free track). On a track,
 * a move later goes latest first, so no layer passes through a neighbour of the group on its way. Clips stay: the
 * main track is magnetic.
 */
export function groupMoveOps(project: Project, m: Members, dtUs: number, rows = 0): Op[] | MoveRefusal {
  const tracks = project.timeline.tracks;
  const moves: { item: Item; from: number; to: Track }[] = [];
  for (const { item, track } of m.layers) {
    if (track.locked) return 'locked';
    if (item.startUs + dtUs < 0) return 'start';
    const from = tracks.indexOf(track);
    const to = tracks[from + rows];
    if (!to || to.locked || to.kind !== track.kind) return to?.locked ? 'locked' : 'kind';
    moves.push({ item, from, to });
  }
  if (dtUs === 0 && rows === 0) return [];
  moves.sort(
    (a, b) =>
      Math.sign(rows) * (b.from - a.from) ||
      (dtUs > 0 ? b.item.startUs - a.item.startUs : a.item.startUs - b.item.startUs),
  );
  return moves.map(({ item, to }) => ({
    type: 'moveItem',
    id: item.id,
    trackId: rows === 0 ? null : to.id,
    startUs: item.startUs + dtUs,
  }));
}

/** A group's move with clips in it: its ops, how far each clip of the main track moves (µs), and the clips' shift. */
export interface GroupReorder {
  ops: Op[];
  /** Each main-track clip that moves, by id: how far its start moves. */
  shifts: Map<string, number>;
  /** How far the group's clips move as a block: its layers that don't follow a clip of it move as far. */
  deltaUs: number;
}

/**
 * A group with clips in it, dragged `dtUs`: its clips go together, in their order, to the place on the main track
 * nearest where their first one would start (the main track is magnetic, so the others close up around them), and its
 * layers that don't follow one of its clips move as far as the clips did, so they keep their places beside them. A drag
 * that doesn't reach another place changes nothing. Refused as a layers-only move is (`groupMoveOps`).
 */
export function groupReorderOps(project: Project, m: Members, dtUs: number): GroupReorder | MoveRefusal {
  const main = project.timeline.main;
  const index = mainIndex(main);
  const picked = new Set(m.clips.map(({ clip }) => clip.id));
  const rest = main.filter((clip) => !picked.has(clip.id));
  const from = index.starts[m.clips[0].index];
  const want = from + dtUs;
  // The places the block can go: before each clip left, or at the end. The nearest to where it was dragged.
  let best = { k: 0, startUs: 0 };
  let start = 0;
  for (let k = 0; k <= rest.length; k++) {
    if (Math.abs(start - want) < Math.abs(best.startUs - want)) best = { k, startUs: start };
    if (k < rest.length) start += clipDurationUs(rest[k]);
  }
  // Still in the place it began (every clip before the first of it isn't the group's): nothing moves, and clips of it
  // that aren't together stay apart.
  if (best.k === m.clips[0].index) return { ops: [], shifts: new Map(), deltaUs: 0 };
  const block = m.clips.map(({ clip }) => clip);
  const order = [...rest.slice(0, best.k), ...block, ...rest.slice(best.k)];
  const deltaUs = best.startUs - from;
  const free: Members = {
    clips: [],
    layers: m.layers.filter(({ item }) => !item.attach || !picked.has(item.attach.clipId)),
  };
  const moves = groupMoveOps(project, free, deltaUs);
  if (typeof moves === 'string') return moves;
  // The new order as moves: each place in turn takes the clip that goes there (a clip already there stays).
  const ops: Op[] = [];
  const now = main.map((clip) => clip.id);
  order.forEach((clip, j) => {
    const k = now.indexOf(clip.id);
    if (k === j) return;
    now.splice(k, 1);
    now.splice(j, 0, clip.id);
    ops.push({ type: 'moveClip', id: clip.id, index: j });
  });
  const shifts = new Map<string, number>();
  let t = 0;
  for (const clip of order) {
    const was = index.starts[main.indexOf(clip)];
    if (t !== was) shifts.set(clip.id, t - was);
    t += clipDurationUs(clip);
  }
  return { ops: [...ops, ...moves], shifts, deltaUs };
}

/**
 * Its blocks for the clipboard, deep copies, each with where it goes from (`GroupEntry`): clips in their order, then
 * layers.
 */
export function groupEntries(project: Project, m: Members): GroupEntry[] {
  const index = mainIndex(project.timeline.main);
  const clipStart = (i: number) => index.starts[m.clips[i].index];
  const first =
    m.clips.length > 0 ? clipStart(0) : Math.min(...m.layers.map(({ item }) => item.startUs));
  return [
    ...m.clips.map(({ clip, index: i }) => ({
      entry: { kind: 'clip' as const, clip: structuredClone(clip) },
      offsetUs: index.starts[i] - first,
    })),
    ...m.layers.map(({ item }) => {
      const on = m.clips.findIndex(({ clip }) => clip.id === item.attach?.clipId);
      return {
        entry: { kind: 'item' as const, item: structuredClone(item) },
        offsetUs: item.startUs - (on >= 0 ? clipStart(on) : first),
        ...(on >= 0 && { onClip: on }),
      };
    }),
  ];
}

/** A copy of a clip with a new id and no transition (its old neighbour isn't beside it). */
function clipCopy(clip: Clip): Clip {
  const copy: Clip = { ...structuredClone(clip), id: newId() };
  delete copy.transitionOut;
  return copy;
}

/** Where a group's copies go: its clips' place on the main track and their start, or `atUs` for a group of layers. */
interface Landing {
  /** The index the first clip copy goes in at. */
  index: number;
  /** Where the first clip copy starts (a group with clips), or where the group's first layer goes (one without). */
  startUs: number;
  /** A layer copy's track: the first free one of its kind (a paste), or its own (a duplicate). */
  track: (from: Track | null) => TrackTarget;
  /** Whether a layer copy that isn't on a clip of the group is attached where it lands. */
  attach: (item: Item, startUs: number, mainEndUs: number) => boolean;
}

/**
 * The copies of `entries` landing at `to`: the clips go in first, in their order, then each layer from its clip's copy
 * (so a title on a copied clip lands on its copy, attached) or from the first clip's copy, latest first so no copy
 * passes through another. Returns the ops and the copies' ids.
 */
function landCopies(
  project: Project,
  entries: readonly (GroupEntry & { track?: Track })[],
  to: Landing,
): [Op[], string[]] {
  const fps = project.settings.fps;
  const ops: Op[] = [];
  const ids: string[] = [];
  const starts: number[] = [];
  let [place, at] = [to.index, to.startUs];
  for (const { entry } of entries) {
    if (entry.kind !== 'clip') continue;
    const clip = clipCopy(entry.clip);
    ops.push({ type: 'insertClip', clip, index: place++ });
    ids.push(clip.id);
    starts.push(at);
    at += clipDurationUs(clip);
  }
  const mainEndUs = mainIndex(project.timeline.main).mainEndUs + (at - to.startUs);
  const from = (e: GroupEntry) => (e.onClip !== undefined ? starts[e.onClip] : to.startUs);
  const layers = entries
    .filter((e) => e.entry.kind === 'item')
    .map((e) => ({ ...e, startUs: Math.max(0, snapToFrame(from(e) + e.offsetUs, fps)) }))
    .sort((a, b) => b.startUs - a.startUs);
  for (const { entry, onClip, startUs, track } of layers) {
    if (entry.kind !== 'item') continue;
    const item = { ...structuredClone(entry.item), id: newId(), startUs, attach: null };
    const attach = onClip !== undefined || to.attach(entry.item, startUs, mainEndUs);
    ops.push({ type: 'addItem', track: to.track(track ?? null), item, attach });
    ids.push(item.id);
  }
  return [ops, ids];
}

/**
 * Paste a copied group: its clips after the clip under the playhead (at the end past it), in their order, and its
 * layers from there as they were from its first clip, a layer on one of its clips on that clip's copy; a group of
 * layers alone at the playhead, offsets kept. Each layer goes on the first free track of its kind, a picture over the
 * video attached there. Returns the ops and the new blocks' ids.
 */
export function groupPasteOps(project: Project, entries: GroupEntry[], playheadUs: number): [Op[], string[]] {
  const at = snapToFrame(playheadUs, project.settings.fps);
  const index = mainIndex(project.timeline.main);
  const under = clipIndexAt(index, at);
  const clips = entries.some((e) => e.entry.kind === 'clip');
  return landCopies(project, entries, {
    index: under >= 0 ? under + 1 : project.timeline.main.length,
    startUs: !clips ? at : under >= 0 ? index.ends[under] : index.mainEndUs,
    track: () => ({ type: 'auto' }),
    attach: (item, startUs, mainEndUs) => item.content.type !== 'audio' && startUs < mainEndUs,
  });
}

/**
 * Duplicate: copies of its clips right after its last clip, in their order, and of its layers from there as they were
 * from its first clip (a layer on one of its clips on that clip's copy); a group of layers alone right after itself.
 * Each layer copy goes on its own track, attached as it was. Layers on locked tracks aren't copied. Returns the ops and
 * the copies' ids.
 */
export function groupDuplicateOps(project: Project, m: Members): [Op[], string[]] {
  const index = mainIndex(project.timeline.main);
  const kept: Members = { clips: m.clips, layers: m.layers.filter(({ track }) => !track.locked) };
  if (countOf(kept) === 0) return [[], []];
  const tracks = new Map(kept.layers.map(({ item, track }) => [item.id, track]));
  const entries = groupEntries(project, kept).map((e) => ({
    ...e,
    track: e.entry.kind === 'item' ? tracks.get(e.entry.item.id) : undefined,
  }));
  const last = kept.clips.at(-1);
  // Its clips' copies right after its last clip; a group of layers alone right after its last layer ends.
  const startUs = last ? index.ends[last.index] : Math.max(...kept.layers.map(({ item }) => itemEndUs(item)));
  return landCopies(project, entries, {
    index: last ? last.index + 1 : project.timeline.main.length,
    startUs,
    track: (from) => (from ? { type: 'track', id: from.id } : { type: 'auto' }),
    attach: (item) => item.attach !== null,
  });
}

/** A group's blocks that make sound and have a volume of their own: clips with sound, sound and video layers. */
export function soundsOf(project: Project, m: Members): Members {
  return {
    clips: m.clips.filter(({ clip }) => clipHasSound(project, clip)),
    layers: m.layers.filter(
      ({ item }) =>
        item.content.type === 'audio' || (item.content.type === 'video' && hasOwnSound(project, item)),
    ),
  };
}

/** Whether every block of a group has a volume and a mute (the group card's sound fields). */
export function allSound(project: Project, m: Members): boolean {
  return countOf(soundsOf(project, m)) === countOf(m);
}

/** Whether every block of a group can carry effects: clips, and layers that draw on overlay tracks. */
export function allEffects(m: Members): boolean {
  return m.layers.every(({ track, item }) => track.kind === 'overlay' && item.content.type !== 'audio');
}

/** Mute (or unmute) every sound of the group not on a locked track, each keeping its volume. */
export function groupMuteOps(project: Project, m: Members, muted: boolean): Op[] {
  const s = soundsOf(project, m);
  return [
    ...s.clips.filter(({ clip }) => clip.muted !== muted).map(({ clip }) => clipOp(clip.id, { muted })),
    ...s.layers
      .filter(({ track, item }) => !track.locked && soundOf(item)?.muted !== muted)
      .flatMap(({ item }) => contentOp(item, { muted })),
  ];
}

/** Every sound of the group at `volume` (0 to 2), not on a locked track. */
export function groupVolumeOps(project: Project, m: Members, volume: number): Op[] {
  const s = soundsOf(project, m);
  return [
    ...s.clips.filter(({ clip }) => clip.volume !== volume).map(({ clip }) => clipOp(clip.id, { volume })),
    ...s.layers
      .filter(({ track, item }) => !track.locked && soundOf(item)?.volume !== volume)
      .flatMap(({ item }) => contentOp(item, { volume })),
  ];
}

/** Whether the group's sounds are all muted (the card's Mute reads on). */
export function groupMuted(project: Project, m: Members): boolean {
  const s = soundsOf(project, m);
  return (
    countOf(s) > 0 &&
    s.clips.every(({ clip }) => clip.muted) &&
    s.layers.every(({ item }) => soundOf(item)?.muted === true)
  );
}

/** The group's sounds' volume when they share one, else null. */
export function groupVolume(project: Project, m: Members): number | null {
  const s = soundsOf(project, m);
  const all = [...s.clips.map(({ clip }) => clip.volume), ...s.layers.map(({ item }) => soundOf(item)?.volume)];
  return all.length > 0 && all.every((v) => v === all[0]) ? (all[0] ?? null) : null;
}

/** Paste effects: the copied stack onto every block of the group that can carry one. */
export function groupPasteEffectsOps(project: Project, m: Members, effects: EffectInstance[]): Op[] {
  return [...m.clips.map(({ clip }) => clip.id), ...m.layers.map(({ item }) => item.id)].flatMap((id) =>
    pasteEffectsOps(project, id, effects),
  );
}

function clipOp(id: string, patch: { muted?: boolean; volume?: number }): Op {
  return { type: 'updateClip', id, patch };
}

function soundOf(item: Item): { muted: boolean; volume: number } | null {
  const c = item.content;
  return c.type === 'audio' || c.type === 'video' ? { muted: c.muted ?? false, volume: c.volume } : null;
}

function contentOp(item: Item, change: { muted?: boolean; volume?: number }): Op[] {
  const c = item.content;
  if (c.type !== 'audio' && c.type !== 'video') return [];
  return [{ type: 'updateItem', id: item.id, patch: { content: { ...c, ...change } } }];
}

/** The ids of every clip and layer on the timeline (⌘A). */
export function everyBlock(project: Project): string[] {
  return [
    ...project.timeline.main.map((clip) => clip.id),
    ...project.timeline.tracks.flatMap((track) => track.items.map((item) => item.id)),
  ];
}

/**
 * ⇧-click: the blocks from `from` to `to` on one track (both clips, or both layers on one track), in its order; null
 * when they aren't on one track.
 */
export function rangeOnTrack(project: Project, from: string, to: string): string[] | null {
  const main = project.timeline.main;
  const [a, b] = [main.findIndex((c) => c.id === from), main.findIndex((c) => c.id === to)];
  if (a >= 0 && b >= 0) return main.slice(Math.min(a, b), Math.max(a, b) + 1).map((c) => c.id);
  const [x, y] = [findItem(project, from), findItem(project, to)];
  if (!x || !y || x.track.id !== y.track.id) return null;
  const items = x.track.items;
  const [i, j] = [items.indexOf(x.item), items.indexOf(y.item)];
  return items.slice(Math.min(i, j), Math.max(i, j) + 1).map((item) => item.id);
}
