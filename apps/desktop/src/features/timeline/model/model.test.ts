import { describe, expect, it } from 'vitest';
import { fixtureProject } from '../../../test/fixtures';
import { longProject } from '../../../test/longProject';
import { blockElement, registerBlock } from '../drag/registry';
import { LAYER_MIN_PX } from '../layout';
import { clipIndexAt, itemSpans, mainIndex } from './index';
import { packRows } from './packRows';
import { bandMask } from '../lanes/Band';
import { mergeTiny, runBands, visibleRange } from './visible';

const S = 1_000_000;

describe('timeline model', () => {
  it('indexes a main track once per array and finds clips by binary search', () => {
    const main = longProject().timeline.main;
    const index = mainIndex(main);
    expect(mainIndex(main)).toBe(index);
    expect(index.mainEndUs).toBe(3_600_000_000);
    expect(clipIndexAt(index, 0)).toBe(0);
    expect(clipIndexAt(index, 2_400_000)).toBe(1);
    expect(clipIndexAt(index, 3_600_000_000)).toBe(-1);
  });

  it('finds the blocks a window shows', () => {
    const index = mainIndex(longProject().timeline.main);
    // 60 to 90 s: clips 25 (60 to 62.4 s) to 37 (88.8 to 91.2 s).
    expect(visibleRange(index, 60_000_000, 90_000_000)).toEqual([25, 38]);
    expect(visibleRange(index, 4e9, 5e9)).toEqual([1500, 1500]);
  });

  it('merges sub-pixel blocks into bands but keeps the selected one', () => {
    const index = mainIndex(longProject().timeline.main);
    // At 0.2 px/s each 2.4 s clip is 0.48 px wide.
    const pieces = mergeTiny(index, 0, 10, 0.2, 2, (i) => i === 5);
    expect(pieces.map((p) => p.kind)).toEqual(['band', 'block', 'band']);
    expect(pieces[1]).toEqual({ kind: 'block', index: 5 });
    // Two tiny layers ten minutes apart are two bands, not one painted over the empty time between them.
    const sparse = { starts: Float64Array.of(0, 600e6), ends: Float64Array.of(1e6, 601e6) };
    expect(mergeTiny(sparse, 0, 2, 0.2, 2, () => false).map((p) => p.kind)).toEqual(['band', 'band']);
  });

  it("draws a row's bands that no block interrupts as one element, keeping their parts apart", () => {
    // A title every 36 s at fit zoom (≈ 0.33 px/s): 9 px apart, so each is its own band; a selected one stays a block.
    const titles = longProject().timeline.tracks[0].items;
    const spans = itemSpans(titles);
    const pieces = mergeTiny(spans, 0, titles.length, 1176 / 3600, LAYER_MIN_PX, (i) => i === 50);
    expect(pieces.filter((p) => p.kind === 'band')).toHaveLength(99);
    const runs = runBands(pieces);
    expect(runs.map((p) => p.kind)).toEqual(['bands', 'block', 'bands']);
    const [before] = runs;
    if (before.kind !== 'bands') throw new Error('a run of bands');
    expect(before.parts).toHaveLength(50);
    expect(before.parts[1]).toEqual([36e6, 44e6]);
    // Its mask paints the parts and leaves the 28 s between them empty.
    expect(
      bandMask([
        [0, 2.6],
        [11.8, 14.4],
      ]),
    ).toBe(
      'linear-gradient(to right, #000 0.0px 2.6px, transparent 2.6px 11.8px, #000 11.8px 14.4px, transparent 14.4px)',
    );
    expect(bandMask([[0, 0.4]])).toBeUndefined();
  });

  it('packs picture layers into at most three rows and counts the rest', () => {
    const p = fixtureProject();
    const item = p.timeline.tracks[0].items[0];
    p.timeline.tracks = [0, 1, 2, 3].map((n) => ({
      ...p.timeline.tracks[0],
      id: `o${n}`,
      items: [{ ...item, id: `i${n}` }],
    }));
    const packing = packRows(p.timeline.tracks);
    expect(packing.rows).toBe(3);
    // The top track wins row 0 at equal starts.
    expect(packing.row.get('i3')).toBe(0);
    expect(packing.row.get('i0')).toBeUndefined();
    expect(packing.overflow).toEqual([
      {
        startUs: item.startUs,
        endUs: item.startUs + item.durationUs,
        count: 1,
        items: [p.timeline.tracks[0].items[0]],
      },
    ]);
  });

  it('lanes_pack_overlays_without_overlap', () => {
    // Two titles and a sticker over the same seconds, on tracks of their own (Auto placement), and one after them.
    const p = fixtureProject();
    const item = p.timeline.tracks[0].items[0];
    const at = (id: string, startUs: number) => ({ ...item, id, startUs, durationUs: 3_000_000 });
    p.timeline.tracks = [
      { ...p.timeline.tracks[0], id: 'o0', items: [at('title', 1_000_000), at('later', 5_000_000)] },
      { ...p.timeline.tracks[0], id: 'o1', items: [at('lower', 2_000_000)] },
      { ...p.timeline.tracks[0], id: 'o2', items: [at('sticker', 3_000_000)] },
    ];
    const packing = packRows(p.timeline.tracks);
    expect(packing.rows).toBe(3);
    // In each row, every layer ends before the next starts.
    for (const line of packing.lines)
      line.items.forEach((a, i) => {
        const b = line.items[i + 1];
        if (b) expect(a.startUs + a.durationUs).toBeLessThanOrEqual(b.startUs);
      });
    // The three that overlap each have a row; the later one shares one.
    expect(new Set(['title', 'lower', 'sticker'].map((id) => packing.row.get(id))).size).toBe(3);
    expect(packing.row.get('later')).toBeDefined();
    expect(packing.overflow).toEqual([]);
  });

  it('merges per packed row, so no band covers a block in its row, a selected one included', () => {
    // A logo on the lower track from 10:05 to 50:00; 3 s titles every 20 s on the track above.
    const p = fixtureProject();
    const [base] = p.timeline.tracks;
    const item = base.items[0];
    const logo = { ...item, id: 'logo', startUs: 605 * S, durationUs: 2395 * S };
    const titles = Array.from({ length: 180 }, (_, i) => ({
      ...item,
      id: `t${i}`,
      startUs: i * 20 * S,
      durationUs: 3 * S,
    }));
    const packing = packRows([
      { ...base, id: 'low', items: [logo] },
      { ...base, id: 'high', items: titles },
    ]);
    // At Fit (≈0.33 px/s) each title is 1 px wide; t40 (13:20, in row 1 beside the logo) is selected.
    const covered = (id: string) => {
      const r = packing.row.get(id)!;
      const { items } = packing.lines[r];
      const it = items.find((x) => x.id === id)!;
      const spans = itemSpans(items);
      const pieces = mergeTiny(spans, 0, items.length, 0.33, LAYER_MIN_PX, (i) => items[i].id === 't40');
      expect(pieces).toContainEqual({ kind: 'block', index: items.indexOf(it) });
      return pieces.some(
        (p) => p.kind === 'band' && p.startUs < it.startUs + it.durationUs && it.startUs < p.endUs,
      );
    };
    expect(packing.row.get('logo')).toBe(0);
    expect(packing.row.get('t40')).toBe(1);
    expect(covered('logo')).toBe(false);
    expect(covered('t40')).toBe(false);
  });

  it('spans items in order', () => {
    const spans = itemSpans(longProject().timeline.tracks[0].items);
    expect(spans.starts[1]).toBe(36_000_000);
    expect(spans.ends[1]).toBe(44_000_000);
  });
});

describe('block registry', () => {
  it('keeps a remounted block when the old element’s cleanup runs after the new one registered', () => {
    const [before, after] = [document.createElement('button'), document.createElement('button')];
    const forgetBefore = registerBlock('x', before);
    registerBlock('x', after);
    forgetBefore();
    expect(blockElement('x')).toBe(after);
  });
});
