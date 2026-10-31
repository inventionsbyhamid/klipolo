import { describe, expect, it } from 'vitest';
import type { WordView } from '../../bindings/WordView';
import { drain, sameSpelling, within, type Queued, type Settled } from './pressQueue';

/** The guarded queue of presses made while the words are being updated. */

const S = 1_000_000;
/** A word of `asset` over `from` to `to` s of it, heard at `at` s. */
const w = (
  text: string,
  from: number,
  to = from + 0.5,
  at = from,
  extra: Partial<WordView> = {},
): WordView => ({
  text,
  joined: false,
  startUs: at * S,
  endUs: (at + to - from) * S,
  assetId: 'mic',
  sourceStartUs: from * S,
  sourceEndUs: to * S,
  look: {},
  keyword: null,
  ...extra,
});
const [so, we, then, go] = [w('so', 0), w('we', 1), w('then', 2), w('go', 3)];

describe('within: every word now was lit when the press was made', () => {
  it('passes the same words, moved on the timeline or with one cut from inside', () => {
    expect(within([so, we, then], [w('so', 0, 0.5, 9), w('we', 1, 1.5, 10)])).toBe(true);
    expect(within([so, we, then], [so, then])).toBe(true);
  });

  it('passes a correction over lit words alone, and the words of a lit correction undone', () => {
    expect(within([we, then], [w('we’n', 1, 2.5)])).toBe(true);
    expect(within([w('we’n', 1, 2.5)], [we, then])).toBe(true);
  });

  it('fails a word moved in, another file’s, and a correction that took in a word not lit', () => {
    expect(within([so, we], [so, go, we])).toBe(false);
    expect(within([so, we], [so, w('we', 1, 1.5, 1, { assetId: 'cam' })])).toBe(false);
    expect(within([we], [w('we’n', 1, 2.5)])).toBe(false);
    expect(within([we, then], [w('we’n go', 1, 3.5)])).toBe(false);
  });

  it('passes a correction whose ends are lit words, whatever lies between them', () => {
    // A `WordView` doesn't list a correction's file words, so an unlit word between two lit ones reads as a pause:
    // the limit `within`'s comment states ("a word not lit at its ends").
    expect(within([so, then], [w('so we’n', 0, 2.5)])).toBe(true);
    expect(within([we, then], [w('so we’n', 0, 2.5)])).toBe(false);
    expect(within([so, we], [w('so we’n', 0, 2.5)])).toBe(false);
  });
});

describe('sameSpelling: a spelling’s words found again as they were typed over', () => {
  it('holds through a ripple, and not when a word changed, hid or was cut from its neighbour', () => {
    expect(sameSpelling([w('we', 1, 1.5, 7), w('then', 2, 2.5, 8)], [we, then])).toBe(true);
    expect(sameSpelling([we, w('then', 2, 2.5, 2, { said: 'than' })], [we, then])).toBe(false);
    expect(sameSpelling([we, w('then', 2, 2.5, 2, { look: { hidden: true } })], [we, then])).toBe(false);
    expect(sameSpelling([we, w('then', 2, 2.5, 2, { cutBefore: true })], [we, then])).toBe(false);
    expect(sameSpelling([w('we’n', 1, 2.5)], [we, then])).toBe(false);
  });
});

describe('drain: what the queue sends once the words settle', () => {
  const pick = {};
  const at = (over: Partial<Settled> = {}): Settled => ({
    picked: pick,
    acts: 2,
    words: [we, then],
    find: (ws) => ws,
    ...over,
  });
  const look = (over: Partial<Extract<Queued, { kind: 'look' }>> = {}): Queued => ({
    kind: 'look',
    change: 'bigger',
    picked: pick,
    acts: 2,
    lit: [we, then],
    fixes: 0,
    ...over,
  });
  const spelling: Queued = { kind: 'spelling', words: [we, then], text: 'we’n', from: 'we then' };

  it('sends every look press in order on the words now', () => {
    const { steps, rest } = drain([look(), look({ change: { color: '#32D74B' } })], at());
    expect(steps).toEqual([
      { kind: 'look', change: 'bigger', words: [we, then] },
      { kind: 'look', change: { color: '#32D74B' }, words: [we, then] },
    ]);
    expect(rest).toEqual([]);
  });

  it('drops a press made on another pick, before another action, or over words now not lit', () => {
    const { steps } = drain(
      [look({ picked: {} }), look({ acts: 1 }), look({ lit: [we] })],
      at({ words: [we, then] }),
    );
    expect(steps.map((s) => s.kind)).toEqual(['dropped', 'dropped', 'dropped']);
    expect(drain([look()], at({ picked: null, words: [] })).steps).toEqual([{ kind: 'dropped' }]);
  });

  it('sends a spelling found again, and then waits for its words to change before what follows', () => {
    const { steps, rest } = drain([spelling, look()], at());
    expect(steps).toEqual([{ kind: 'spelling', item: spelling, words: [we, then] }]);
    expect(rest).toEqual([look()]);
  });

  it('lets a look sent first land before a spelling after it', () => {
    const { steps, rest } = drain([look(), spelling], at());
    expect(steps.map((s) => s.kind)).toEqual(['look']);
    expect(rest).toEqual([spelling]);
  });

  it('says a spelling wasn’t saved when its words aren’t one run as typed, and goes on', () => {
    const { steps, rest } = drain([spelling, look()], at({ find: () => null }));
    expect(steps).toEqual([
      { kind: 'unsaved', item: spelling },
      { kind: 'look', change: 'bigger', words: [we, then] },
    ]);
    expect(rest).toEqual([]);
    expect(drain([spelling], at({ find: () => [w('we’n', 1, 2.5)] })).steps[0].kind).toBe('unsaved');
  });
});
