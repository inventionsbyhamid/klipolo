import { describe, expect, it } from 'vitest';
import type { WordView } from '../../bindings/WordView';
import { findAgain, follow, indexOf, wordKey, type Listed, type Sel } from './wordSelection';

/** Finding a selection's words again after an edit. */

const S = 1_000_000;
/** A page word heard at `at` s from the mic's `from` to `to` s (half a second unless given). */
function w(text: string, at: number, from: number, to = from + 0.5): WordView {
  return {
    text,
    joined: false,
    startUs: at * S,
    endUs: (at + 0.5) * S,
    assetId: 'mic',
    sourceStartUs: from * S,
    sourceEndUs: to * S,
    look: {},
    keyword: null,
  };
}
const listed = (words: WordView[]): Listed[] => words.map((word, row) => ({ key: wordKey(word), word, row }));
const run = (now: Listed[], old: Listed[]) => follow(now, indexOf(now), old);

describe('a selection’s words found again', () => {
  const before = listed([w('so', 0, 0), w('we', 1, 1), w('then', 2, 2), w('go', 3, 3)]);
  const old = before.slice(1, 3);

  it('follows them through a ripple, and into the correction they became', () => {
    expect(run(listed([w('so', 0, 0), w('we', 0.6, 1), w('then', 1.6, 2), w('go', 2.6, 3)]), old)).toEqual([
      1, 2,
    ]);
    // "we then" fixed to one word: the correction's range holds both.
    expect(run(listed([w('so', 0, 0), w('we-then', 1, 1, 2.5), w('go', 3, 3)]), old)).toEqual([1, 1]);
  });

  it('says when they moved apart or are gone', () => {
    expect(run(listed([w('so', 0, 0), w('we', 1, 1), w('go', 2, 3), w('then', 3, 2)]), old)).toBe('apart');
    expect(run(listed([w('so', 0, 0), w('we', 1, 1), w('go', 2, 3)]), old)).toBe('missing');
  });

  it('takes the copy nearest where each word played', () => {
    // The clip copied to the end: the second copy is 10 s later.
    const now = listed([w('we', 1, 1), w('then', 2, 2), w('x', 3, 7), w('we', 11, 1), w('then', 12, 2)]);
    const late = listed([w('we', 11.2, 1), w('then', 12.2, 2)]);
    expect(run(now, late)).toEqual([3, 4]);
  });
});

describe('a correction the selection followed into, undone', () => {
  // "we then go" fixed to one word over the mic's 1 to 3.5 s, then undone.
  const fixed = listed([w('so', 0, 0), w('wego', 1, 1, 3.5), w('now', 4, 4)]);
  const undone = listed([w('so', 0, 0), w('we', 1, 1), w('then', 2, 2), w('go', 3, 3), w('now', 4, 4)]);

  it('stands for the words the correction was made of, and stops at the word picked at its end', () => {
    expect(follow(undone, indexOf(undone), fixed.slice(1, 2))).toEqual([1, 3]);
    const go = undone[3].word;
    expect(follow(undone, indexOf(undone), fixed.slice(1, 2), [go, go])).toEqual([3, 3]);
    // Inside the selection it stands for all of them.
    expect(follow(undone, indexOf(undone), fixed, [fixed[0].word, fixed[2].word])).toEqual([0, 4]);
  });

  it('never takes a copy played right after a word for part of it', () => {
    const copied = listed([w('so', 0, 0), w('we', 1, 1), w('we', 2, 1), w('go', 3, 3)]);
    expect(follow(copied, indexOf(copied), listed([w('we', 1.2, 1)]))).toEqual([1, 1]);
  });
});

describe('a selection found again after an edit', () => {
  /** The selection from `a` to `b` (picked so), and its words. */
  const selOf = (words: Listed[]): Sel => {
    const [a, b] = [words[0], words[words.length - 1]];
    return { anchor: a.key, anchorRow: a.row, focus: b.key, focusRow: b.row, picked: [a.word, b.word] };
  };
  const answer = (l: Listed[]) => [l, indexOf(l)] as const;

  it('keeps the rest of its words when one inside it is cut (an "um")', () => {
    const before = listed([w('so', 0, 0), w('um', 1, 1), w('we', 2, 2), w('go', 3, 3)]);
    const sel = selOf(before);
    const now = listed([w('so', 0, 0), w('we', 1, 2), w('go', 2, 3)]);
    const found = findAgain([answer(now)], sel, { sel, chosen: before });
    expect(found !== null && found !== 'missing' && found.run.map((l) => l.word.text)).toEqual([
      'so',
      'we',
      'go',
    ]);
  });

  it('takes the words picked where they lie nearest where they were, though a copy is in the other answer', () => {
    const picked = listed([w('we', 1, 1), w('then', 2, 2)]);
    // Chat removed a pause (all 0.4 s earlier) before the pick's answer came; the window holds the clip's copy.
    const copy = listed([w('we', 11.6, 1), w('then', 12.6, 2)]);
    const own = listed([w('we', 0.6, 1), w('then', 1.6, 2)]);
    const found = findAgain([answer(copy), answer(own)], selOf(picked), { sel: null, chosen: [] });
    expect(found !== null && found !== 'missing' && found.run.map((l) => l.word.startUs)).toEqual([
      0.6 * S,
      1.6 * S,
    ]);
  });
});

describe('finding a long selection again', () => {
  it('re-finds 200 pages of 32 words after a ripple in a few milliseconds, by an index, not a scan', () => {
    // Every key changed (all words 0.4 s earlier): each word is found by its file word.
    const words = (n: number, shift: number) =>
      listed(Array.from({ length: n }, (_, i) => w('w', i - shift, i)));
    const [old, now] = [words(200 * 32, 0), words(220 * 32, 0.4)];
    for (let k = 0; k < 3; k++) run(now, old);
    const t0 = performance.now();
    const found = run(now, old);
    const took = performance.now() - t0;
    expect(found).toEqual([0, 200 * 32 - 1]);
    // Indexing the answer and finding them: a scan of the answer per word took about 0.3 s here; this, about 8 ms. The
    // budget leaves room for a loaded test machine (56 ms once, and 51 ms on a busy gate) and still catches
    // the scan.
    expect(took).toBeLessThan(150);
  });
});
