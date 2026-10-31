import { describe, expect, it } from 'vitest';
import type { Op } from '../../../bindings/Op';
import { fixtureProject } from '../../../test/fixtures';
import { dropAllOps, dropOps, nearestCut } from './dropOps';

const p = fixtureProject();
const asset = (id: string) => p.assets.find((a) => a.id === id)!;

describe('dropping media', () => {
  it('a video on the main track is inserted at the cut', () => {
    const ops = dropOps(p, asset('broll'), { lane: 'main', index: 1 }, 30);
    expect(ops).toHaveLength(1);
    expect(ops[0]).toMatchObject({
      type: 'insertClip',
      index: 1,
      clip: { source: { type: 'asset', id: 'broll' } },
    });
  });

  it('a video on Layers becomes an attached layer at the drop time, on the frame grid', () => {
    const [op] = dropOps(p, asset('broll'), { lane: 'picture', trackId: null, atUs: 2_010_000 }, 30);
    expect(op).toMatchObject({
      type: 'addItem',
      track: { type: 'auto' },
      attach: true,
      item: { startUs: 2_000_000, durationUs: 10_000_000, content: { type: 'video', assetId: 'broll' } },
    });
  });

  it('a video layer comes in 40 % of the frame wide, its own shape, in the bottom right of every format', () => {
    // The b-roll is 1920 × 1080: in 16:9, 40 % of the width and of the height, 4 % of the short side in from the right
    // and bottom edges. It is drawn contained in its box, so a box of its own shape shows all of it and nothing else.
    const [op] = dropOps(p, asset('broll'), { lane: 'picture', trackId: null, atUs: 0 }, 30);
    const layout = op.type === 'addItem' ? op.item.layout : null;
    const margin = 0.04 * 1080;
    expect(layout?.anchor).toBe('bottomRight');
    expect(layout?.rect.w).toBeCloseTo(0.4);
    expect(layout?.rect.h).toBeCloseTo(0.4);
    expect(layout?.rect.x).toBeCloseTo(1 - 0.4 - margin / 1920);
    expect(layout?.rect.y).toBeCloseTo(1 - 0.4 - margin / 1080);
    // Its own place in 9:16: 40 % wide, inside what the apps leave clear (the "all" safe area: x 6 % to 87 %, y 14 % to
    // 74 %), its bottom right corner the margin in from that area's.
    const portrait = layout?.overrides.portrait;
    expect(portrait?.w).toBeCloseTo(0.4);
    expect(portrait?.h).toBeCloseTo((0.4 * 1080 * (1080 / 1920)) / 1920);
    expect(portrait!.x + portrait!.w).toBeCloseTo(0.87 - margin / 1080);
    expect(portrait!.y + portrait!.h).toBeCloseTo(0.74 - margin / 1920);
    // In 1:1, 40 % wide too; the primary format has no override of its own.
    expect(layout?.overrides.square?.w).toBeCloseTo(0.4);
    expect(layout?.overrides.landscape).toBeUndefined();
  });

  it('a tall video layer is 40 % of the frame high, and a 9:16 project places it inside the safe area', () => {
    const tall = { ...asset('broll'), id: 'phone', width: 1080, height: 1920 };
    const [op] = dropOps(p, tall, { lane: 'picture', trackId: null, atUs: 0 }, 30);
    const rect = op.type === 'addItem' ? op.item.layout.rect : null;
    // 40 % of 1080 px high, 9:16 wide: 432 × 243 px.
    expect(rect?.h).toBeCloseTo(0.4);
    expect(rect?.w).toBeCloseTo(243 / 1920);
    const q = fixtureProject();
    q.settings = { ...q.settings, formats: ['portrait'], primaryFormat: 'portrait' };
    const [inPortrait] = dropOps(q, asset('broll'), { lane: 'picture', trackId: null, atUs: 0 }, 30);
    const layout = inPortrait.type === 'addItem' ? inPortrait.item.layout : null;
    expect(layout?.rect.w).toBeCloseTo(0.4);
    expect(layout!.rect.x + layout!.rect.w).toBeCloseTo(0.87 - (0.04 * 1080) / 1080);
    expect(layout!.rect.y + layout!.rect.h).toBeCloseTo(0.74 - (0.04 * 1080) / 1920);
    expect(layout?.overrides.portrait).toBeUndefined();
    expect(layout?.overrides.landscape?.w).toBeCloseTo(0.4);
  });

  it('a picture keeps its place: medium and centred', () => {
    const [op] = dropOps(p, asset('logo'), { lane: 'picture', trackId: null, atUs: 0 }, 30);
    const layout = op.type === 'addItem' ? op.item.layout : null;
    expect(layout?.anchor).toBe('center');
    expect(layout?.rect.x).toBeCloseTo(0.3);
    expect(layout?.rect.w).toBeCloseTo(0.4);
    expect(layout?.overrides).toEqual({});
  });

  it('an image dropped on the main track becomes a 5 s layer there', () => {
    const [op] = dropOps(p, asset('logo'), { lane: 'main', index: 1 }, 30);
    expect(op).toMatchObject({
      type: 'addItem',
      attach: true,
      item: { startUs: 20_000_000, durationUs: 5_000_000, content: { type: 'image' } },
    });
  });

  it('sound stays free and goes on the track it was dropped on', () => {
    const [op] = dropOps(p, asset('music'), { lane: 'sound', trackId: 't2', atUs: 3_000_000 }, 30);
    expect(op).toMatchObject({
      type: 'addItem',
      track: { type: 'track', id: 't2' },
      attach: false,
      item: { startUs: 3_000_000 },
    });
  });

  it('on the new-track zone, the layer gets a track of its own, appended in the same batch', () => {
    const ops = dropOps(p, asset('music'), { lane: 'sound', trackId: null, atUs: 0, newTrack: true }, 30);
    expect(ops).toMatchObject([
      { type: 'addTrack', kind: 'audio', index: 2 },
      { type: 'addItem', attach: false },
    ]);
    const [add, item] = ops;
    expect(item).toMatchObject({ track: { type: 'track', id: add.type === 'addTrack' ? add.id : '' } });
  });

  it('a layer never goes on a track that would refuse it: another kind, or locked', () => {
    // A video dropped on an audio track is a picture layer (auto), and so is one on a locked overlay track.
    const [onSound] = dropOps(p, asset('broll'), { lane: 'sound', trackId: 't2', atUs: 0 }, 30);
    expect(onSound).toMatchObject({ type: 'addItem', track: { type: 'auto' }, attach: true });
    const locked = fixtureProject();
    locked.timeline.tracks[0].locked = true;
    const [onLocked] = dropOps(locked, asset('logo'), { lane: 'picture', trackId: 't1', atUs: 0 }, 30);
    expect(onLocked).toMatchObject({ track: { type: 'auto' } });
  });

  it('a layer at an off-grid cut starts exactly on it: never a frame early over the clip before it', () => {
    // The first clip ends at 10 010 010 µs, off the 30 fps grid (imported clips end at their file's own length).
    const q = fixtureProject();
    q.timeline.main[0].sourceOutUs = 10_010_010;
    const startOf = (ops: Op[]) => (ops[0].type === 'addItem' ? ops[0].item.startUs : -1);
    // On the main track at that cut: the cut's time, not rounded.
    expect(startOf(dropOps(q, asset('logo'), { lane: 'main', index: 1 }, 30))).toBe(10_010_010);
    // Dropped (or added at a playhead) within half a frame of it: the cut, exactly. Rounded, this was 10 000 000.
    expect(
      startOf(dropOps(q, asset('broll'), { lane: 'picture', trackId: null, atUs: 10_010_010 }, 30)),
    ).toBe(10_010_010);
    expect(startOf(dropOps(q, asset('logo'), { lane: 'picture', trackId: 't1', atUs: 10_000_000 }, 30))).toBe(
      10_010_010,
    );
    expect(startOf(dropOps(q, asset('music'), { lane: 'sound', trackId: 't2', atUs: 10_020_000 }, 30))).toBe(
      10_010_010,
    );
    // A frame further away, the frame grid.
    expect(startOf(dropOps(q, asset('logo'), { lane: 'picture', trackId: null, atUs: 9_966_667 }, 30))).toBe(
      9_966_667,
    );
  });

  it('a layer’s edge just after a cut wins over the cut; one just before it loses', () => {
    // A cut at 10.02 s, off the 30 fps grid, and the title on t1 ending on the next frame (10 033 333 µs): 13 ms apart.
    const q = fixtureProject();
    q.timeline.main[0].sourceOutUs = 10_020_000;
    const title = q.timeline.tracks[0].items[0];
    title.durationUs = 10_033_333 - title.startUs;
    const startOf = (ops: Op[]) => (ops[0].type === 'addItem' ? ops[0].item.startUs : -1);
    // Snapped to the title's end: it meets the title, rather than starting at the cut over its last 13 ms (which
    // would lift it to the next track up).
    expect(startOf(dropOps(q, asset('logo'), { lane: 'picture', trackId: 't1', atUs: 10_033_333 }, 30))).toBe(
      10_033_333,
    );
    // An edge before the cut, within half a frame of it, still loses: the layer would show over the clip before.
    title.durationUs = 10_010_000 - title.startUs;
    expect(startOf(dropOps(q, asset('logo'), { lane: 'picture', trackId: 't1', atUs: 10_010_000 }, 30))).toBe(
      10_020_000,
    );
  });

  it('a drop snapped to a layer’s off-grid edge meets it exactly; files dropped together follow on exactly', () => {
    const q = fixtureProject();
    // The title (1 s) now ends at 4 012 345 µs.
    q.timeline.tracks[0].items[0].durationUs = 3_012_345;
    const [op] = dropOps(q, asset('logo'), { lane: 'picture', trackId: 't1', atUs: 4_012_345 }, 30);
    expect(op).toMatchObject({ item: { startUs: 4_012_345 } });
    // A clip 10 010 010 µs long, then an image after it on the same track.
    const long = { ...asset('broll'), id: 'long', durationUs: 10_010_010 };
    const ops = dropAllOps(q, [long, asset('logo')], { lane: 'picture', trackId: 't1', atUs: 5_000_000 }, 30);
    expect(ops.slice(1).map((o) => (o.type === 'addItem' ? o.item.startUs : -1))).toEqual([
      5_000_000, 15_010_010,
    ]);
  });

  it('a hidden track takes no layer: it goes where it shows', () => {
    const q = fixtureProject();
    q.timeline.tracks[0].hidden = true;
    const [op] = dropOps(q, asset('logo'), { lane: 'picture', trackId: 't1', atUs: 0 }, 30);
    expect(op).toMatchObject({ track: { type: 'auto' } });
  });

  it('the main track takes the cut nearest the pointer, the end after the last clip', () => {
    // Cuts at 0, 20 s and 30 s.
    const main = p.timeline.main;
    expect(
      [0, 9_000_000, 11_000_000, 24_000_000, 26_000_000, 90_000_000].map((us) => nearestCut(main, us)),
    ).toEqual([0, 0, 1, 1, 2, 2]);
    expect(nearestCut([], 5_000_000)).toBe(0);
  });

  it('several files dropped at once: videos in order at the cut, each layer after the one before', () => {
    const files = [asset('broll'), asset('logo'), asset('screen')];
    const main = dropAllOps(p, files, { lane: 'main', index: 1 }, 30);
    expect(main.map((op) => op.type)).toEqual(['addAssets', 'insertClip', 'addItem', 'insertClip']);
    // The logo lands at the cut after the b-roll (20 s + 10 s), the screen recording after the b-roll.
    expect(main[2]).toMatchObject({ item: { startUs: 30_000_000 } });
    expect(main[3]).toMatchObject({ index: 2 });
    const layers = dropAllOps(p, files, { lane: 'picture', trackId: 't1', atUs: 1_000_000 }, 30);
    expect(layers.slice(1).map((op) => (op.type === 'addItem' ? op.item.startUs : -1))).toEqual([
      1_000_000, 11_000_000, 16_000_000,
    ]);
  });
});
