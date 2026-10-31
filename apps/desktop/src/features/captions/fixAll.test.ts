import { describe, expect, it } from 'vitest';
import type { WordView } from '../../bindings/WordView';
import { fixAllOps } from './WordEditor';

const S = 1_000_000;
const at = (said: string, s: number): WordView => ({
  text: said.toUpperCase(),
  said,
  joined: false,
  startUs: s * S,
  endUs: (s + 0.5) * S,
  assetId: 'mic',
  sourceStartUs: s * S,
  sourceEndUs: (s + 0.5) * S,
  look: {},
  keyword: null,
});
const texts = (ops: ReturnType<typeof fixAllOps>) => ops.map((op) => (op.type === 'fixWords' ? op.text : null));

describe('Fix all', () => {
  const mid = at('the', 1);
  const start = at('The', 4);

  it('gives each place the first-letter case of its own word, against the word edited', () => {
    expect(texts(fixAllOps(mid, [mid, start], 'a'))).toEqual(['a', 'A']);
    expect(texts(fixAllOps(start, [mid, start], 'A'))).toEqual(['a', 'A']);
  });

  it('keeps the marks around each place, and the edited word exactly as typed', () => {
    const ending = at('the.', 7);
    expect(texts(fixAllOps(start, [start, ending], 'A'))).toEqual(['A', 'a.']);
    expect(texts(fixAllOps(mid, [mid, start], 'a, really'))).toEqual(['a, really', 'A, really']);
  });

  it('leaves the case alone where the word edited starts with no letter', () => {
    const number = at('3d', 2);
    const other = at('3D', 5);
    expect(texts(fixAllOps(number, [number, other], '3-d'))).toEqual(['3-d', '3-d']);
  });
});
