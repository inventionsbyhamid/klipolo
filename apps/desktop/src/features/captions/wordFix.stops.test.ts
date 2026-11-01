import { describe, expect, it } from 'vitest';
import type { Op } from '../../bindings/Op';
import type { WordView } from '../../bindings/WordView';
import { fixOps, joinWords, visibleWords } from './wordFix';

/**
 * "Fix the words" never covers a hidden word, two files' words, or file time played elsewhere: a fix stops there, and
 * words taken out where no fix can take them in are hidden instead.
 */

const S = 1_000_000;
type Fix = Extract<Op, { type: 'fixWords' }>;
type Look = Extract<Op, { type: 'setWordLook' }>;

/** A page word heard at `at` s from second `src` of file `file` (half a second each). */
function word(text: string, at: number, src = at, file = 'a', hidden = false, cut = false): WordView {
  return {
    text,
    joined: false,
    startUs: at * S,
    endUs: at * S + S / 2,
    assetId: file,
    sourceStartUs: src * S,
    sourceEndUs: src * S + S / 2,
    look: hidden ? { hidden: true } : {},
    keyword: null,
    ...(cut ? { cutBefore: true } : {}),
  };
}

/**
 * Page words from "text", "text@b" (file b, else a), "(text)" (hidden), "text:10" (from the file's 10 s, else from its
 * place) and "|text" (a cut before it), word i heard at i s.
 */
function page(spec: string): WordView[] {
  return spec.split(' ').map((part, i) => {
    const m = /^(\|?)(\(?)([^@:)]+)\)?(?:@(\w+))?(?::(\d+))?$/.exec(part)!;
    return word(m[3], i, m[5] ? Number(m[5]) : i, m[4] ?? 'a', m[2] === '(', m[1] === '|');
  });
}

/** Whether `w` lies inside `[start, end)` of its file, by its middle (as a look or a fix finds it). */
const inside = (w: WordView, asset: string, start: number, end: number) => {
  const mid = (w.sourceStartUs + w.sourceEndUs) / 2;
  return w.assetId === asset && start <= mid && mid < end;
};

/**
 * What the page reads once `ops` are applied as the model applies them (a fix replaces its file's fixes it overlaps
 * and reads once over its words; a look hides the words in its ranges), with what went wrong: a fix over a hidden
 * word, a fix whose words have another file's words between them, two fixes that overlap, or a fix over words of
 * `elsewhere` (words of the files played elsewhere in the video, whose captions it would take).
 */
function reads(
  all: WordView[],
  ops: Op[],
  elsewhere: WordView[] = [],
): { text: string; wrong: string | null } {
  let fixes: Fix[] = [];
  let wrong: string | null = null;
  const hidden = new Set(all.filter((w) => w.look.hidden));
  for (const op of ops) {
    if (op.type === 'fixWords') {
      const meets = (f: Fix) => f.assetId === op.assetId && f.endUs > op.startUs && f.startUs < op.endUs;
      if (fixes.some(meets)) wrong ??= 'two fixes overlap';
      fixes = [...fixes.filter((f) => !meets(f)), op];
      const covered = all.flatMap((w, i) => (inside(w, op.assetId, op.startUs, op.endUs) ? [i] : []));
      if (covered.some((i) => hidden.has(all[i]))) wrong ??= `"${op.text}" covers a hidden word`;
      const [a, b] = [covered[0], covered.at(-1)!];
      if (all.slice(a, b + 1).some((w) => w.assetId !== op.assetId)) wrong ??= `"${op.text}" mixes two files`;
      if (elsewhere.some((w) => inside(w, op.assetId, op.startUs, op.endUs)))
        wrong ??= `"${op.text}" covers words played elsewhere`;
    } else {
      const look = op as Look;
      for (const w of all)
        if (look.look.hidden && look.ranges.some((r) => inside(w, look.assetId, r.start, r.end)))
          hidden.add(w);
    }
  }
  const units: string[] = [];
  let last: Fix | null = null;
  for (const w of all) {
    if (hidden.has(w)) continue;
    const f = fixes.find((x) => inside(w, x.assetId, x.startUs, x.endUs)) ?? null;
    if (!f) units.push(w.text);
    else if (f !== last) units.push(f.text ?? '');
    last = f;
  }
  return { text: units.join(' '), wrong };
}

/** What `typed` sends over `spec`'s page as [kind, text or file, first word, last word], checked to read as typed. */
function saved(spec: string, typed: string, elsewhere: WordView[] = []) {
  const all = page(spec);
  const ops = fixOps(all, typed);
  expect(reads(all, ops, elsewhere)).toEqual({ text: typed, wrong: null });
  return ops.map((op) =>
    op.type === 'fixWords'
      ? ['fix', op.text, op.startUs / S, Math.round((op.endUs - S / 2) / S)]
      : ['hide', (op as Look).assetId, ...(op as Look).ranges.map((r) => r.start / S)],
  );
}

describe('Fix the words stops at a hidden word and at another file’s words', () => {
  it('fixes a word next to a hidden one alone, and never across it', () => {
    expect(saved('so (um) now we', 'so then we')).toEqual([['fix', 'then', 2, 2]]);
    // Two words made one across the hidden "um": the text goes before it, and "now", with no word of its own side to
    // join, is hidden.
    expect(saved('so (um) now', 'alright')).toEqual([
      ['fix', 'alright', 0, 0],
      ['hide', 'a', 2],
    ]);
    // A word taken out goes with its own side's neighbour.
    expect(saved('so (um) now we go', 'so we go')).toEqual([['fix', 'we', 2, 3]]);
  });

  it('keeps each file’s words to its own fixes', () => {
    expect(saved('hello world@b', 'hey')).toEqual([
      ['fix', 'hey', 0, 0],
      ['hide', 'b', 1],
    ]);
    expect(saved('one two three@b four@b', 'one two three five')).toEqual([['fix', 'five', 3, 3]]);
    expect(saved('one two@b', 'one more two')).toEqual([['fix', 'one more', 0, 0]]);
    expect(saved('w x y@b z@b', 'w z')).toEqual([
      ['fix', 'w', 0, 1],
      ['fix', 'z', 2, 3],
    ]);
  });

  it('stops at a cut, so a fix never takes the captions of words played elsewhere', () => {
    // The mic's 2 to 10 s moved to the end of the video: the page at the cut reads "so we | then go" (the file's 0, 1,
    // then 10, 11 s). Deleting "then", or changing it, must leave the moved words' captions alone.
    const moved = Array.from({ length: 8 }, (_, k) => word('moved', 20 + k, 2 + k));
    expect(saved('so we |then:10 go:11', 'so we go', moved)).toEqual([['fix', 'go', 10, 11]]);
    // Two words made one across the cut: the text goes before it, and the word after it joins its own side's.
    expect(saved('so we |then:10 go:11', 'so next go', moved)).toEqual([
      ['fix', 'next', 1, 1],
      ['fix', 'go', 10, 11],
    ]);
    expect(saved('so we |then:10 go:11', 'so we now go', moved)).toEqual([['fix', 'now', 10, 10]]);
    // A cut before a hidden word between them stops a fix too.
    expect(saved('so we |(um):5 then:10 go:11', 'so we go', moved)).toEqual([['fix', 'go', 10, 11]]);
  });

  it('reads as typed, never covering a hidden word, mixing files or taking words played elsewhere, over 3,000 random edits (fixed seed)', () => {
    // mulberry32: its state steps by an odd constant, so it repeats only after 2^32 draws.
    let state = 2810;
    const rnd = () => {
      state = (state + 0x6d2b79f5) | 0;
      let t = Math.imul(state ^ (state >>> 15), 1 | state);
      t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
      return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
    };
    const pick = <T>(xs: T[]) => xs[Math.floor(rnd() * xs.length)];
    const vocab = 'the cat sat on a so um go big red now yes'.split(' ');
    const failures: string[] = [];
    for (let n = 0; n < 3000; n++) {
      const two = rnd() < 0.5;
      const blocks = rnd() < 0.5;
      const count = 1 + Math.floor(rnd() * 9);
      const said = Array.from({ length: count }, (_, i) => ({
        text: pick(vocab),
        src: i,
        file: !two ? 'a' : (blocks ? i >= count / 2 : rnd() < 0.5) ? 'b' : 'a',
        hidden: rnd() < 0.2,
      }));
      // Every third page is clips of the file in shuffled order, a clip of them played elsewhere in the video.
      let runs = [said];
      if (n % 3 === 0) {
        runs = [];
        for (let i = 0; i < count; i++)
          if (i === 0 || rnd() < 0.35) runs.push([said[i]]);
          else runs.at(-1)!.push(said[i]);
        for (let i = runs.length - 1; i > 0; i--) {
          const j = Math.floor(rnd() * (i + 1));
          [runs[i], runs[j]] = [runs[j], runs[i]];
        }
      }
      const away = runs.length > 1 && rnd() < 0.5 ? runs.splice(Math.floor(rnd() * runs.length), 1)[0] : [];
      // A cut before each clip's first word, as `captions_pages` says it; another file is a cut too.
      const all = runs
        .flatMap((run, r) => run.map((w, k) => ({ ...w, cut: r > 0 && k === 0 })))
        .map((w, i, ws) =>
          word(w.text, i, w.src, w.file, w.hidden, w.cut || (i > 0 && ws[i - 1].file !== w.file)),
        );
      const elsewhere = away.map((w) => word(w.text, 100 + w.src, w.src, w.file));
      const spec = all
        .map((w) => `${w.cutBefore ? '|' : ''}${w.text}@${w.assetId}:${w.sourceStartUs / S}`)
        .join(' ');
      if (visibleWords(all).length === 0) continue;
      // Words put in, taken out, replaced or swapped, or characters anywhere, spaces too.
      const items = joinWords(visibleWords(all)).split(' ');
      let typed: string;
      if (n % 2 === 0) {
        for (let e = 1 + Math.floor(rnd() * 3); e > 0; e--) {
          const [op, at] = [Math.floor(rnd() * 4), Math.floor(rnd() * (items.length + 1))];
          if (op === 0) items.splice(at, 0, pick(vocab));
          else if (op === 1 && items.length > 1 && at < items.length) items.splice(at, 1);
          else if (op === 2 && at < items.length) items[at] = pick(vocab);
          else if (op === 3 && at + 1 < items.length) [items[at], items[at + 1]] = [items[at + 1], items[at]];
        }
        typed = items.join(' ');
      } else {
        const cp = [...items.join(' ')];
        for (let e = 1 + Math.floor(rnd() * 3); e > 0; e--) {
          const [op, at] = [Math.floor(rnd() * 3), Math.floor(rnd() * (cp.length + 1))];
          if (op === 0) cp.splice(at, 0, pick([...'ab x']));
          else if (op === 1) cp.splice(at, 1);
          else if (at < cp.length) cp[at] = pick([...'ab x']);
        }
        typed = cp.join('');
      }
      const want = typed.trim().replace(/\s+/g, ' ');
      if (!want) continue;
      const got = reads(all, fixOps(all, typed), elsewhere);
      const why = got.wrong ?? (got.text !== want ? `reads "${got.text}"` : null);
      if (why) failures.push(`${spec} → ${typed}: ${why}`);
    }
    expect(failures.slice(0, 5)).toEqual([]);
  });
});
