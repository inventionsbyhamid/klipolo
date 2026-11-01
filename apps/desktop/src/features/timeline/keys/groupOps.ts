import type { Clip } from '../../../bindings/Clip';
import type { EffectInstance } from '../../../bindings/EffectInstance';
import type { Item } from '../../../bindings/Item';
import type { Op } from '../../../bindings/Op';
import type { Project } from '../../../bindings/Project';
import type { Track } from '../../../bindings/Track';
import { newId } from '../../../lib/defaults';
import { snapToFrame } from '../../../lib/time';
import { clipHasSound, findItem, hasOwnSound, itemEndUs } from '../../../lib/timeline';
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
 * go to is missing, locked or of another kind, or when one would start before 0:00. A move later goes latest first,
 * so no layer passes through a neighbour of the group on its way. Clips stay: the main track is magnetic.
 */
export function groupMoveOps(project: Project, m: Members, dtUs: number, rows = 0): Op[] | MoveRefusal {
  const tracks = project.timeline.tracks;
  const moves: { item: Item; to: Track }[] = [];
  for (const { item, track } of m.layers) {
    if (track.locked) return 'locked';
    if (item.startUs + dtUs < 0) return 'start';
    const to = tracks[tracks.indexOf(track) + rows];
    if (!to || to.locked || to.kind !== track.kind) return to?.locked ? 'locked' : 'kind';
    moves.push({ item, to });
  }
  if (dtUs === 0 && rows === 0) return [];
  moves.sort((a, b) => (dtUs > 0 ? b.item.startUs - a.item.startUs : a.item.startUs - b.item.startUs));
  return moves.map(({ item, to }) => ({
    type: 'moveItem',
    id: item.id,
    trackId: rows === 0 ? null : to.id,
    startUs: item.startUs + dtUs,
  }));
}

/** Its blocks for the clipboard, deep copies, each at its distance from the earliest one (clips in their order). */
export function groupEntries(project: Project, m: Members): GroupEntry[] {
  const index = mainIndex(project.timeline.main);
  const starts = [
    ...m.clips.map(({ index: i }) => index.starts[i]),
    ...m.layers.map(({ item }) => item.startUs),
  ];
  const first = Math.min(...starts);
  return [
    ...m.clips.map(({ clip, index: i }) => ({
      entry: { kind: 'clip' as const, clip: structuredClone(clip) },
      offsetUs: index.starts[i] - first,
    })),
    ...m.layers.map(({ item }) => ({
      entry: { kind: 'item' as const, item: structuredClone(item) },
      offsetUs: item.startUs - first,
    })),
  ];
}

/** A copy of a clip with a new id and no transition (its old neighbour isn't beside it). */
function clipCopy(clip: Clip): Clip {
  const copy: Clip = { ...structuredClone(clip), id: newId() };
  delete copy.transitionOut;
  return copy;
}

/**
 * Paste a copied group at the playhead: its layers at the playhead plus their offsets (each on the first free track of
 * its kind, a picture over the video attached there), its clips after the clip under the playhead, in their order.
 * Returns the ops and the new blocks' ids.
 */
export function groupPasteOps(project: Project, entries: GroupEntry[], playheadUs: number): [Op[], string[]] {
  const at = snapToFrame(playheadUs, project.settings.fps);
  const index = mainIndex(project.timeline.main);
  const under = clipIndexAt(index, at);
  let place = under >= 0 ? under + 1 : project.timeline.main.length;
  const ops: Op[] = [];
  const ids: string[] = [];
  for (const { entry, offsetUs } of entries) {
    if (entry.kind === 'clip') {
      const clip = clipCopy(entry.clip);
      ops.push({ type: 'insertClip', clip, index: place++ });
      ids.push(clip.id);
      continue;
    }
    const startUs = snapToFrame(at + offsetUs, project.settings.fps);
    const item = { ...structuredClone(entry.item), id: newId(), startUs, attach: null };
    const attach = item.content.type !== 'audio' && startUs < index.mainEndUs;
    ops.push({ type: 'addItem', track: { type: 'auto' }, item, attach });
    ids.push(item.id);
  }
  return [ops, ids];
}

/**
 * Duplicate: a copy of each layer right after the group, on its own track (attached like it), and of each clip right
 * after the group's last clip, in their order. Layers on locked tracks aren't copied. Returns the ops and the copies'
 * ids.
 */
export function groupDuplicateOps(project: Project, m: Members): [Op[], string[]] {
  const index = mainIndex(project.timeline.main);
  const spans = [
    ...m.clips.map(({ index: i }) => [index.starts[i], index.ends[i]]),
    ...m.layers.map(({ item }) => [item.startUs, itemEndUs(item)]),
  ];
  const span = Math.max(...spans.map(([, e]) => e)) - Math.min(...spans.map(([s]) => s));
  const ops: Op[] = [];
  const ids: string[] = [];
  const free = m.layers.filter(({ track }) => !track.locked);
  // Latest first, so a copy never lands where a copy before it in the batch would bump it.
  free.sort((a, b) => b.item.startUs - a.item.startUs);
  for (const { item, track } of free) {
    const copy = { ...structuredClone(item), id: newId(), startUs: item.startUs + span, attach: null };
    ops.push({ type: 'addItem', track: { type: 'track', id: track.id }, item: copy, attach: item.attach !== null });
    ids.push(copy.id);
  }
  let place = (m.clips.at(-1)?.index ?? -1) + 1;
  for (const { clip } of m.clips) {
    const copy = clipCopy(clip);
    ops.push({ type: 'insertClip', clip: copy, index: place++ });
    ids.push(copy.id);
  }
  return [ops, ids];
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
