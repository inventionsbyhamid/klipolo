import { describe, expect, it } from 'vitest';
import type { Op } from '../../../bindings/Op';
import type { Project } from '../../../bindings/Project';
import { fixtureProject, titleItem } from '../../../test/fixtures';
import {
  allEffects,
  allSound,
  groupDeleteOps,
  groupDuplicateOps,
  groupEntries,
  groupMoveOps,
  groupMuteOps,
  groupPasteOps,
  groupReorderOps,
  groupVolumeOps,
  membersOf,
  rangeOnTrack,
} from './groupOps';

const S = 1_000_000;

/** The fixture with a second title at 6 s on the title's track. */
function twoTitles(): Project {
  const p = fixtureProject();
  p.timeline.tracks[0].items.push({ ...titleItem, id: 'second', startUs: 6 * S, durationUs: S, attach: null });
  return p;
}

/**
 * Overlay tracks o1 to o5 (bottom to top) alone on the timeline: l1 on o2 from 3 s, l2 on o3 from 2 s and l3 on o4 from
 * 1 s, each 5 s long, so all three overlap.
 */
function stacked(): Project {
  const p = fixtureProject();
  const at = (id: string, start: number) => ({ ...titleItem, id, startUs: start * S, durationUs: 5 * S, attach: null });
  const lane = { ...p.timeline.tracks[0], items: [] };
  p.timeline.tracks = [
    { ...lane, id: 'o1', name: 'Overlay 1' },
    { ...lane, id: 'o2', name: 'Overlay 2', items: [at('l1', 3)] },
    { ...lane, id: 'o3', name: 'Overlay 3', items: [at('l2', 2)] },
    { ...lane, id: 'o4', name: 'Overlay 4', items: [at('l3', 1)] },
    { ...lane, id: 'o5', name: 'Overlay 5' },
  ];
  return p;
}

/**
 * Whether each `moveItem` of `ops`, applied in turn, lands on a track with nothing in its way: where the model would
 * otherwise bump the layer to another track.
 */
function landsFree(p: Project, ops: Op[]): boolean {
  const tracks = p.timeline.tracks.map((t) => ({ id: t.id, items: t.items.map((i) => ({ ...i })) }));
  for (const op of ops) {
    if (op.type !== 'moveItem') continue;
    const from = tracks.find((t) => t.items.some((i) => i.id === op.id))!;
    const item = from.items.find((i) => i.id === op.id)!;
    from.items = from.items.filter((i) => i !== item);
    const to = tracks.find((t) => t.id === (op.trackId ?? from.id))!;
    const end = op.startUs + item.durationUs;
    if (to.items.some((i) => i.startUs < end && op.startUs < i.startUs + i.durationUs)) return false;
    to.items.push({ ...item, startUs: op.startUs });
  }
  return true;
}

describe("a group's edits", () => {
  it('deletes its layers, then takes its clips out in one deleteRanges', () => {
    const p = fixtureProject();
    expect(groupDeleteOps(p, membersOf(p, ['b', 'title', 'song']))).toEqual([
      { type: 'removeItem', id: 'title' },
      { type: 'removeItem', id: 'song' },
      { type: 'deleteRanges', ranges: [{ start: 20 * S, end: 30 * S }] },
    ]);
  });

  it('moves its layers together, the latest first going later, and refuses what can’t go', () => {
    const p = twoTitles();
    const m = membersOf(p, ['title', 'second']);
    expect(groupMoveOps(p, m, S)).toEqual([
      { type: 'moveItem', id: 'second', trackId: null, startUs: 7 * S },
      { type: 'moveItem', id: 'title', trackId: null, startUs: 2 * S },
    ]);
    expect(groupMoveOps(p, m, -2 * S)).toBe('start');
    // The audio track is the next one up from the titles' track: a title can't go there.
    expect(groupMoveOps(p, m, 0, 1)).toBe('kind');
    p.timeline.tracks[0].locked = true;
    expect(groupMoveOps(p, membersOf(p, ['title', 'second']), S)).toBe('locked');
  });

  it('moves its layers across tracks the top one first going up and the bottom one first going down', () => {
    const p = stacked();
    const m = membersOf(p, ['l1', 'l2', 'l3']);
    // Up a track and 1 s later: each lands on the track the one above it has just left.
    const up = groupMoveOps(p, m, S, 1);
    expect(up).toEqual([
      { type: 'moveItem', id: 'l3', trackId: 'o5', startUs: 2 * S },
      { type: 'moveItem', id: 'l2', trackId: 'o4', startUs: 3 * S },
      { type: 'moveItem', id: 'l1', trackId: 'o3', startUs: 4 * S },
    ]);
    expect(landsFree(p, up as Op[])).toBe(true);
    // Down a track and 1 s earlier: the bottom one first.
    const down = groupMoveOps(p, m, -S, -1);
    expect(down).toEqual([
      { type: 'moveItem', id: 'l1', trackId: 'o1', startUs: 2 * S },
      { type: 'moveItem', id: 'l2', trackId: 'o2', startUs: 1 * S },
      { type: 'moveItem', id: 'l3', trackId: 'o3', startUs: 0 },
    ]);
    expect(landsFree(p, down as Op[])).toBe(true);
    // Up with no time change, and down: still none in another's way.
    expect(landsFree(p, groupMoveOps(p, m, 0, 1) as Op[])).toBe(true);
    expect(landsFree(p, groupMoveOps(p, m, 0, -1) as Op[])).toBe(true);
  });

  it('carries its clips together to the nearest other place on the main track, and the others close up', () => {
    const p = fixtureProject();
    const [a] = p.timeline.main;
    p.timeline.main.push({ ...a, id: 'c', sourceInUs: 30 * S, sourceOutUs: 40 * S });
    // 5 s later, a would still go first: nothing changes, and a and c stay apart.
    expect(groupReorderOps(p, membersOf(p, ['a', 'c']), 5 * S)).toEqual({ ops: [], shifts: new Map(), deltaUs: 0 });
    // 8 s later, after b: a and c go there together.
    expect(groupReorderOps(p, membersOf(p, ['a', 'c']), 8 * S)).toEqual({
      ops: [{ type: 'moveClip', id: 'b', index: 0 }],
      shifts: new Map([
        ['b', -20 * S],
        ['a', 10 * S],
      ]),
      deltaUs: 10 * S,
    });
  });

  it('pastes a copied group at the playhead with its offsets, and duplicates it right after itself', () => {
    const p = twoTitles();
    const m = membersOf(p, ['title', 'second']);
    const [ops, ids] = groupPasteOps(p, groupEntries(p, m), 10 * S);
    expect(ids).toHaveLength(2);
    // The latest first, so no copy passes through another.
    expect(ops.map((op) => (op.type === 'addItem' ? op.item.startUs : -1))).toEqual([15 * S, 10 * S]);
    const [copies] = groupDuplicateOps(p, m);
    // The group runs 1 to 7 s: the copies go 6 s later, the latest first.
    expect(copies.map((op) => (op.type === 'addItem' ? op.item.startUs : -1))).toEqual([12 * S, 7 * S]);
  });

  it('mutes and sets the volume of its sounds, and says which fields they share', () => {
    const p = fixtureProject();
    const sounds = membersOf(p, ['a', 'song']);
    expect(allSound(p, sounds)).toBe(true);
    expect(allEffects(sounds)).toBe(false);
    expect(groupMuteOps(p, sounds, true)).toEqual([
      { type: 'updateClip', id: 'a', patch: { muted: true } },
      { type: 'updateItem', id: 'song', patch: { content: { ...p.timeline.tracks[1].items[0].content, muted: true } } },
    ]);
    expect(groupVolumeOps(p, sounds, 0.4)).toEqual([{ type: 'updateClip', id: 'a', patch: { volume: 0.4 } }]);
    const pictures = membersOf(p, ['b', 'title']);
    expect(allSound(p, pictures)).toBe(false);
    expect(allEffects(pictures)).toBe(true);
  });

  it('a ⇧-click range runs along one track only', () => {
    const p = twoTitles();
    expect(rangeOnTrack(p, 'a', 'b')).toEqual(['a', 'b']);
    expect(rangeOnTrack(p, 'second', 'title')).toEqual(['title', 'second']);
    expect(rangeOnTrack(p, 'a', 'title')).toBeNull();
  });
});
