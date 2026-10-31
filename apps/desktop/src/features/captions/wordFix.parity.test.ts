import { describe, expect, it } from 'vitest';
import edgeKept from '../../lib/caption-edge-kept.json';
import parity from '../../lib/caption-join-parity.json';
import { styleById } from '../../lib/captionStyles';
import { drawnJoined, joinsUnspaced, shownEdges } from './captionJoins';

/** `caption-join-parity.json`, written by `crates/klipolo-model/tests/pages.rs`. */
const fixture = parity as {
  unspaced: [number, number][];
  pairs: { style: string; said: [string, string]; shown: [string, string]; joined: boolean }[];
};

/** The code points (surrogates aside) where `ours` and the runs Rust wrote disagree, the first ten in hex. */
function differ(runs: [number, number][], ours: (c: string) => boolean): string[] {
  const out: string[] = [];
  let k = 0;
  for (let c = 0; c <= 0x10ffff && out.length < 10; c++) {
    if (c >= 0xd800 && c <= 0xdfff) continue;
    while (k < runs.length && runs[k][1] < c) k++;
    const inRust = k < runs.length && runs[k][0] <= c;
    if (ours(String.fromCodePoint(c)) !== inRust) out.push(c.toString(16));
  }
  return out;
}

describe('how "Fix the words" spaces a fix’s edges, as the captions do (caption-join-parity.json, written by Rust)', () => {
  it('joins the same scripts without spaces as `joins_unspaced`, over every code point', () => {
    expect(fixture.unspaced.length).toBeGreaterThan(0);
    expect(differ(fixture.unspaced, joinsUnspaced)).toEqual([]);
  });

  it('keeps at a word’s edge exactly what a style that shows only ? and ! keeps (`edge_kept`), over every code point', () => {
    // Rust's table, from its own Unicode: the webview's may be older or newer.
    expect(edgeKept.unicode).toMatch(/^\d+\.\d+\.\d+$/);
    expect(edgeKept.kept.length).toBeGreaterThan(100);
    const kept = (c: string) => shownEdges(c, 'questionAndExclamation') !== '';
    expect(differ(edgeKept.kept as [number, number][], kept)).toEqual([]);
    // What a word's edge drops and keeps, by name: the sign before a digit too.
    expect(
      ['C++', '“OK”', '(笑)', '-5', '#1', '…'].map((w) => shownEdges(w, 'questionAndExclamation')),
    ).toEqual(['C++', 'OK', '笑', '-5', '#1', '']);
  });

  it('shows two words and draws them joined or apart as the pages do, in Clean, Boxed and Bold', () => {
    expect(new Set(fixture.pairs.map((p) => p.style))).toEqual(new Set(['clean', 'boxed', 'bold']));
    for (const { style: key, said, shown, joined } of fixture.pairs) {
      const style = styleById(`klipolo.builtin/caption.${key}`)!;
      const name = `${key}: ${said.join(' ')}`;
      const cased = (s: string) => (style.case === 'upper' ? s.toUpperCase() : s);
      expect(
        said.map((s) => cased(shownEdges(s, style.punctuation))),
        name,
      ).toEqual(shown);
      expect(drawnJoined(said[0], said[1], style.punctuation), name).toBe(joined);
    }
  });
});
