import { describe, expect, it } from 'vitest';
import type { CaptionPunctuation } from '../../bindings/CaptionPunctuation';
import type { Op } from '../../bindings/Op';
import type { WordView } from '../../bindings/WordView';
import parity from '../../lib/caption-join-parity.json';
import { shownEdges } from './captionJoins';
import { fixOps, joinWords } from './wordFix';

const S = 1_000_000;
type Fix = Extract<Op, { type: 'fixWords' }>;

/** The scripts the model writes without spaces, as the runs Rust wrote (`speech::joins_unspaced`). */
const UNSPACED: number[][] = parity.unspaced;
const unspaced = (c: string | undefined) => {
  const u = c?.codePointAt(0);
  return u !== undefined && UNSPACED.some(([a, b]) => a <= u && u <= b);
};
/** Whether the model draws `b` right after `a`, without a space (`ends_joining` and `starts_joining`). */
const joins = (a: string, b: string) => unspaced([...a].at(-1)) && unspaced([...b][0]);

/**
 * A caption style as these tests page it: Keep (Clean, Karaoke, Boxed) shows each word as said; Bold (also Arcade, Loud
 * and Highlight) upper-cases it and trims the marks at its edges (`edges_trimmed`, pinned by the parity fixture).
 */
type Style = 'keep' | 'bold';
const PUNCT: Record<Style, CaptionPunctuation> = { keep: 'keep', bold: 'questionAndExclamation' };
const shownIn = (said: string, style: Style) => shownEdges(said, PUNCT[style]);

/**
 * Page words from `texts` (as said), word i heard over the file's `i..i + 0.5` s, as the model pages them in `style`
 * (`paging.rs`): a word the style shows nothing of is left out, and a word is joined when the shown text before it
 * ends, and its own starts, in a script written without spaces.
 */
function page(texts: string[], style: Style = 'keep'): WordView[] {
  const out: WordView[] = [];
  let before: string | null = null;
  texts.forEach((said, i) => {
    const shown = shownIn(said, style);
    if (!shown) return;
    const text = style === 'bold' ? shown.toUpperCase() : shown;
    out.push({
      text,
      ...(text !== said ? { said } : {}),
      joined: before !== null && joins(before, shown),
      startUs: i * S,
      endUs: i * S + S / 2,
      assetId: 'mic',
      sourceStartUs: i * S,
      sourceEndUs: i * S + S / 2,
      look: {},
      keyword: null,
    });
    before = shown;
  });
  return out;
}

/** What `fixOps` sends in `style`, as [text, first word, last word]. */
function fixes(words: WordView[], typed: string, style: Style = 'keep'): [string, number, number][] {
  return fixOps(words, typed, PUNCT[style]).map((op) => {
    if (op.type !== 'fixWords') throw new Error(op.type);
    return [op.text ?? '', op.startUs / S, Math.round((op.endUs - S / 2) / S)];
  });
}

/**
 * What the page reads once `ops` are applied in turn as the model applies them (`fix_words`: a fix replaces every fix
 * of its file it overlaps; a fix reads once, over the words inside its range), spaced as `style` draws it: on what it
 * shows of each, and a fix it shows nothing of (`blank`) left out. And whether one fix replaced another.
 */
function shows(
  words: WordView[],
  ops: Op[],
  style: Style = 'keep',
): { text: string; overlap: boolean; blank: boolean } {
  let kept: Fix[] = [];
  let overlap = false;
  for (const op of ops) {
    if (op.type !== 'fixWords') throw new Error(op.type);
    const meets = (f: Fix) => f.endUs > op.startUs && f.startUs < op.endUs;
    overlap ||= kept.some(meets);
    kept = [...kept.filter((f) => !meets(f)), op];
  }
  const units: string[] = [];
  let last = -1;
  for (const w of words) {
    const k = kept.findIndex((f) => w.sourceStartUs >= f.startUs && w.sourceEndUs <= f.endUs);
    if (k < 0) units.push(w.said ?? w.text);
    else if (k !== last) units.push(kept[k].text ?? '');
    last = k;
  }
  let [text, blank] = ['', false];
  let before: string | null = null;
  for (const u of units) {
    const shown = shownIn(u, style);
    blank ||= !shown;
    if (!shown) continue;
    text += (before !== null && !joins(before, shown) ? ' ' : '') + u;
    before = shown;
  }
  return { text, overlap, blank };
}

/** The fixes `typed` sends in `style`, and that the page then shows exactly what was typed. */
function saved(words: WordView[], typed: string, style: Style = 'keep') {
  const ops = fixOps(words, typed, PUNCT[style]);
  expect(shows(words, ops, style)).toEqual({ text: typed, overlap: false, blank: false });
  return fixes(words, typed, style);
}

/** `a` and `b` read alike in `style`: Bold spaces the marks it hides its own way. */
function alike(a: string, b: string, style: Style): boolean {
  const mark = (c: string | undefined) => c !== undefined && c !== ' ' && !shownIn(c, style);
  const spaced = (s: string) =>
    [...s].filter((c, i, cs) => c !== ' ' || !(mark(cs[i - 1]) || mark(cs[i + 1]))).join('');
  return style === 'keep' ? a === b : spaced(a) === spaced(b);
}

/**
 * The fixes in `ops` that take an unchanged word in for nothing: a fix over two or more words that could leave out its
 * first or last word, and the page would read just the same. Only a word with nothing put in beside it: a first word
 * followed by the next word's own text, a last word after a space or after the word before's own text (what was put
 * in next to a word goes with it, as the diff assigns it). A fix whose text has a different number of words between
 * spaces than the field over its words is a word split or joined by a space: that stretch is fixed whole.
 */
function sweeps(words: WordView[], ops: Op[], style: Style): string[] {
  const out: string[] = [];
  const reading = shows(words, ops, style).text;
  ops.forEach((op, k) => {
    if (op.type !== 'fixWords') return;
    const text = op.text ?? '';
    const i = words.findIndex((w) => w.sourceStartUs === op.startUs);
    let j = words.length - 1;
    while (j >= 0 && words[j].sourceEndUs !== op.endUs) j--;
    if (i < 0 || j <= i || joinWords(words.slice(i, j + 1)).split(' ').length !== text.split(' ').length)
      return;
    const reads = (alt: Fix) => {
      if (!alt.text || alt.text !== alt.text.trim()) return false;
      const got = shows(
        words,
        ops.map((o, m) => (m === k ? alt : o)),
        style,
      );
      return !got.overlap && got.text === reading;
    };
    const said = (w: WordView) => w.said ?? w.text;
    const [first, last] = [said(words[i]), said(words[j])];
    for (const sep of [' ', '']) {
      const [after, before] = [text.slice((first + sep).length), text.slice(0, -(sep + last).length)];
      const whole = [
        after.startsWith(said(words[i + 1])),
        sep === ' ' || before.endsWith(said(words[j - 1])),
      ];
      if (
        text.startsWith(first + sep) &&
        whole[0] &&
        reads({ ...op, startUs: words[i + 1].sourceStartUs, text: after })
      )
        out.push(`${text} without ${first}`);
      if (
        text.endsWith(sep + last) &&
        whole[1] &&
        reads({ ...op, endUs: words[j - 1].sourceEndUs, text: before })
      )
        out.push(`${text} without ${last}`);
    }
  });
  return out;
}

const words = (text: string) => page(text.split(' '));

describe('Fix the words: only what changed, word by word', () => {
  it('keeps letters added or taken at a word’s edge in that word, and two changes as two fixes', () => {
    expect(fixes(words('the cat sat on'), 'the cats sat in')).toEqual([
      ['cats', 1, 1],
      ['in', 3, 3],
    ]);
    expect(fixes(words('I want to go there'), 'I wanted to go here')).toEqual([
      ['wanted', 1, 1],
      ['here', 4, 4],
    ]);
    expect(fixes(words('I wanted to go here'), 'I want to go there')).toEqual([
      ['want', 1, 1],
      ['there', 4, 4],
    ]);
    expect(fixes(words('use the new editor and its tools'), 'use the new editors and its tool set')).toEqual([
      ['editors', 3, 3],
      ['tool set', 6, 6],
    ]);
    expect(fixes(words('the cat sat'), 'a cat unsat')).toEqual([
      ['a', 0, 0],
      ['unsat', 2, 2],
    ]);
  });

  it('keeps two separate changes apart when one adds a word', () => {
    expect(fixes(words('the cat sat on the mat'), 'the cats sat on a big mat')).toEqual([
      ['cats', 1, 1],
      ['a big', 4, 4],
    ]);
    expect(fixes(words('big red car'), 'small blue car')).toEqual([
      ['small', 0, 0],
      ['blue', 1, 1],
    ]);
  });

  it('finds the words inside a Chinese or Japanese run, which has no spaces', () => {
    const ja = page(['今日', 'は', 'いい', '天気']);
    expect(fixes(ja, '今日は良い天気')).toEqual([['良い', 2, 2]]);
    expect(fixes(ja, '明日は良い天気')).toEqual([
      ['明日', 0, 0],
      ['良い', 2, 2],
    ]);
  });

  it('fixes one word, joins a word taken out or put in to its neighbour, and sends nothing for no change', () => {
    expect(fixes(words('Everyone, welcome back.'), 'Everyone, welcome black.')).toEqual([['black.', 2, 2]]);
    expect(fixes(words('so um yes'), 'so yes')).toEqual([['so', 0, 1]]);
    expect(fixes(words('I go'), 'I really go')).toEqual([['I really', 0, 0]]);
    expect(fixes(words('the cat sat'), '  the cat   sat ')).toEqual([]);
    expect(fixes(words('the cat sat'), '   ')).toEqual([]);
  });
});

describe('Fix the words: what is saved is what was typed, and no two fixes overlap', () => {
  it('fixes swapped words and a change next to a word put in or taken out at the page’s start once', () => {
    expect(saved(words('big red car'), 'red big car')).toEqual([
      ['red', 0, 0],
      ['big', 1, 1],
    ]);
    expect(saved(words('one two'), 'two one')).toEqual([
      ['two', 0, 0],
      ['one', 1, 1],
    ]);
    expect(saved(words('um so we start'), 'so now we start')).toEqual([
      ['so', 0, 0],
      ['now', 1, 1],
    ]);
    expect(saved(words('um so yes'), 'so really yes')).toEqual([
      ['so', 0, 0],
      ['really', 1, 1],
    ]);
    expect(saved(words('um so yes'), 'so')).toEqual([['so', 0, 2]]);
    expect(saved(words('so yes'), 'well so really yes')).toEqual([['well so really', 0, 0]]);
    expect(saved(words('so we go'), 'well so go')).toEqual([
      ['well', 0, 0],
      ['so', 1, 1],
    ]);
  });

  it('puts a word taken out of a Chinese or Japanese run into the fix next to it', () => {
    expect(saved(page(['我们', '今天', '那个', '去', '学校']), '我们明天去学校')).toEqual([['明天', 1, 2]]);
    expect(saved(page(['今日', 'は', 'いい', '天気']), '明日いい天気')).toEqual([['明日', 0, 1]]);
    expect(saved(page(['他们', '那个', '今天', '去', '学校']), '你们今天去学校')).toEqual([['你们', 0, 1]]);
    expect(saved(page(['的', '明天', 'は']), '明明天は')).toEqual([['明', 0, 0]]);
  });

  it('keeps a replaced word or character in place, so the words around it keep their reveal', () => {
    expect(saved(words('go to the shop'), 'go the the shop')).toEqual([['the', 1, 1]]);
    expect(saved(page(['去', '明天', '的', '天気']), '去明天的的気')).toEqual([['的気', 3, 3]]);
    // A word taken out next to a run goes with the run's nearest word, not the whole run.
    expect(saved(page(['今日', 'は', 'いい', '天気', 'iPhone']), '今日はいい天気')).toEqual([['天気', 3, 4]]);
    expect(saved(page(['iPhone', 'の', 'カメラ', 'は', 'いい']), 'のカメラはいい')).toEqual([['の', 0, 1]]);
  });

  it('takes the next word in when a fix would change whether the captions put a space between them', () => {
    // A Latin letter inside a Japanese run: "いいX" alone would be drawn with a space before 天気.
    expect(saved(page(['今日', 'は', 'いい', '天気']), '今日はいいX天気')).toEqual([['いいX天気', 2, 3]]);
    // Two Han words the person keeps apart: "今日" alone would be drawn joined to 天気.
    expect(saved(page(['iPhone', '天気']), '今日 天気')).toEqual([['今日 天気', 0, 1]]);
  });
});

describe('Fix the words: a fix’s edges are spaced as the captions draw them', () => {
  it('judges an edge on what was typed there: a word taken out leaves the space before it', () => {
    // A particle taken out after a Latin word: the words after it keep their own reveal.
    expect(saved(page(['iPhone', 'の', 'カメラ', 'は', 'いい']), 'iPhone カメラはいい')).toEqual([
      ['iPhone', 0, 1],
    ]);
    expect(saved(page(['Mac', 'で', '動画', 'を', '作る']), 'Mac 動画を作る')).toEqual([['Mac', 0, 1]]);
    expect(saved(page(['我们', '用', 'iPhone', '的', '相机']), '我们用 iPhone 相机')).toEqual([
      ['iPhone', 2, 3],
    ]);
    // A space typed between two Han words, where a particle stood: the word after comes in to keep it.
    expect(saved(page(['iPhone', 'の', '天気']), '今日 天気')).toEqual([['今日 天気', 0, 2]]);
    expect(saved(page(['a', '的', '今天']), 'は 今天')).toEqual([['は 今天', 0, 2]]);
  });

  it('judges an edge on what the style shows: Bold hides a mark at a word’s edge', () => {
    // "iPhone、" shows IPHONE in Bold, so カメラ is drawn after a space, as typed.
    const camera = (first: string) => page([first, 'カメラ', 'は', 'いい'], 'bold');
    expect(saved(camera('iPhone、'), 'iPad、 カメラはいい', 'bold')).toEqual([['iPad、', 0, 0]]);
    expect(saved(camera('iphone、'), 'iPhone、 カメラはいい', 'bold')).toEqual([['iPhone、', 0, 0]]);
    expect(
      saved(page(['我们', '用', 'iphone，', '拍', '视频'], 'bold'), '我们用 iPhone， 拍视频', 'bold'),
    ).toEqual([['iPhone，', 2, 2]]);
    // The word after a hidden mark fixed.
    expect(saved(page(['今日', 'は', 'Mac。', '明日', 'は'], 'bold'), '今日は Mac。 今日は', 'bold')).toEqual(
      [['今日', 3, 3]],
    );
  });

  it('leaves the space next to a mark the style hides to the style, and the word after keeps its reveal', () => {
    // Bold draws 天気 and 明日 joined, as everywhere a hidden 。 stands between two Japanese words.
    const ws = page(['天気。', 'iPhone'], 'bold');
    expect(fixes(ws, '天気。 明日', 'bold')).toEqual([['明日', 1, 1]]);
    expect(shows(ws, fixOps(ws, '天気。 明日', 'questionAndExclamation'), 'bold').text).toBe('天気。明日');
  });

  it('reads as typed in Keep and Bold, with no two fixes overlapping and no word taken in for nothing, over 4,000 random edits (fixed seed)', () => {
    // 32-bit arithmetic: a product past 2^53 would lose its low bits, and the sequence would repeat within 15,000.
    let seed = 2710;
    const rnd = () => {
      seed = (Math.imul(seed, 1103515245) + 12345) & 0x7fffffff;
      return seed / 0x80000000;
    };
    const pick = <T>(xs: T[]) => xs[Math.floor(rnd() * xs.length)];
    const latin = 'the cat sat on a so um go big red now cats Hello, OK! 😀'.split(' ');
    const cjk = '今日 は いい 天気 です。 カメラ を で の 我们 今天， 那个 去 的 相机'.split(' ');
    // Brand names between scripts, and marks as speech-to-text writes them: on the word before.
    const mixed = [...cjk, ...'iPhone iPhone、 iphone， Mac。 Mac AI 「カメラ」 今日、 the 😀'.split(' ')];
    const chars = [...'ab 今日は天的😀の、。，!'];
    const failures: string[] = [];
    for (let n = 0; n < 4000; n++) {
      const style: Style = n % 4 < 2 ? 'keep' : 'bold';
      const vocab = n % 3 === 0 ? latin : n % 3 === 1 ? cjk : mixed;
      const texts = Array.from({ length: 1 + Math.floor(rnd() * 6) }, () => pick(vocab));
      const ws = page(texts, style);
      let typed: string;
      if (n % 2 === 0) {
        // Words put in (spaced as the captions space them), taken out with the space before them or the one after,
        // replaced, swapped, or given a letter, a particle or a mark; every other space stays as the field has it.
        const items = ws.map((w, i) => ({ t: w.said ?? w.text, sep: i > 0 && !w.joined ? ' ' : '' }));
        for (let e = 1 + Math.floor(rnd() * 3); e > 0; e--) {
          const [op, at] = [Math.floor(rnd() * 5), Math.floor(rnd() * (items.length + 1))];
          if (op === 0) {
            const t = pick(vocab);
            items.splice(at, 0, { t, sep: at > 0 && !joins(items[at - 1].t, t) ? ' ' : '' });
          } else if (op === 1 && items.length > 1 && at < items.length) {
            const [gone] = items.splice(at, 1);
            if (at < items.length && rnd() < 0.5) items[at].sep = gone.sep;
          } else if (op === 2 && at < items.length) items[at].t = pick(vocab);
          else if (op === 3 && at + 1 < items.length)
            [items[at].t, items[at + 1].t] = [items[at + 1].t, items[at].t];
          else if (op === 4 && at < items.length) items[at].t += pick(['s', 'の', '、']);
        }
        typed = items.map((it, i) => (i > 0 ? it.sep : '') + it.t).join('');
      } else {
        // Characters put in, taken out or replaced anywhere, spaces and marks too.
        const cp = [...joinWords(ws)];
        for (let e = 1 + Math.floor(rnd() * 3); e > 0; e--) {
          const [op, at] = [Math.floor(rnd() * 3), Math.floor(rnd() * (cp.length + 1))];
          if (op === 0) cp.splice(at, 0, pick(chars));
          else if (op === 1) cp.splice(at, 1);
          else if (at < cp.length) cp[at] = pick(chars);
        }
        typed = cp.join('');
      }
      if (ws.length === 0) continue;
      const want = typed.trim().replace(/\s+/g, ' ') || joinWords(ws);
      const ops = fixOps(ws, typed, PUNCT[style]);
      const got = shows(ws, ops, style);
      // A fix the style shows nothing of (a lone mark in Bold) isn't on the page to judge.
      const wrong = got.overlap
        ? 'two fixes overlap'
        : !got.blank && !alike(got.text, want, style)
          ? got.text
          : null;
      const why = wrong ?? (got.blank ? undefined : sweeps(ws, ops, style)[0]);
      if (why) failures.push(`${style}: ${texts.join('|')} → ${typed}: ${why}`);
    }
    expect(failures.slice(0, 5)).toEqual([]);
  });
});
