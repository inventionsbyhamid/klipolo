import type { CaptionPunctuation } from '../../bindings/CaptionPunctuation';
import type { Op } from '../../bindings/Op';
import type { WordView } from '../../bindings/WordView';
import { drawnJoined, hidesMarkBetween } from './captionJoins';
import { fixOp, lookOps } from './captionOps';

/**
 * "Fix the words": what the field shows of a page, and the smallest fixes that turn it into what the person typed.
 * The model draws a correction as one word that can't wrap and is revealed at once, so each fix must cover only the
 * words that changed: a page fixed whole would lose its word-by-word reveal, its keyword and its wrap in a style like
 * Bold. Whatever is typed, the page then shows exactly that, its spaces included (beside a mark the style hides, the
 * style spaces it), and no two fixes overlap (the model keeps only the later of two that do). A fix never covers a
 * hidden word or two files' words: it stops there.
 */

/** A word as the field shows it: what was said, or the person's fix (never the style's capitals). */
const saidOf = (w: WordView) => w.said ?? w.text;

/** The words a page draws (hidden ones stay out of the field, so a fix never covers them). */
export const visibleWords = (words: WordView[]) => words.filter((w) => !w.look.hidden);

/** `words` joined as the page joins them: a space before each but the first, unless its script joins it (CJK). */
export function joinWords(words: WordView[], pick: (w: WordView) => string = saidOf): string {
  return words.map((w, i) => (i > 0 && !w.joined ? ' ' : '') + pick(w)).join('');
}

/** The most characters a fix holds (the model's `speech::FIX_MAX_CHARS`). */
export const FIX_MAX_CHARS = 200;

/** What the field starts from, and where each word sits in it. */
function field(words: WordView[]): { text: string; spans: [number, number][] } {
  let text = '';
  const spans: [number, number][] = [];
  words.forEach((w, i) => {
    if (i > 0 && !w.joined) text += ' ';
    const at = text.length;
    text += saidOf(w);
    spans.push([at, text.length]);
  });
  return { text, spans };
}

/**
 * Where a fix stops between a page's visible words (`visibleWords(all)`): after word k when word k + 1 comes from
 * another file, a cut lies before it or before a hidden word between them (`cutBefore`: another clip, whose file time
 * between may play elsewhere), a hidden word lies between them, or it doesn't follow word k in their file. One fix
 * over both would cover the hidden word (and take its look away, or hide the whole fix), write one file's words over
 * the other's, or take the captions of words played elsewhere.
 */
function stops(all: WordView[]): boolean[] {
  const out: boolean[] = [];
  let [prev, apart] = [null as WordView | null, false];
  for (const w of all) {
    apart ||= prev !== null && w.cutBefore === true;
    if (w.look.hidden) {
      apart ||= prev !== null;
      continue;
    }
    if (prev) out.push(apart || w.assetId !== prev.assetId || w.sourceStartUs < prev.sourceEndUs);
    [prev, apart] = [w, false];
  }
  return out;
}

/**
 * `e` cut at each stop it reaches over (`stops`): the part over the words before a stop keeps the text put in; each
 * part after takes out what it covers, the space before its words included. An insertion touches one word: never cut.
 */
function cutAtStops(e: Edit, spans: [number, number][], stop: boolean[]): Edit[] {
  const out: Edit[] = [];
  let [x0, text] = [e.x0, e.text];
  for (let k = 0; k < stop.length && e.x1 > e.x0; k++)
    if (stop[k] && x0 < spans[k][1] && e.x1 > spans[k + 1][0]) {
      out.push({ x0, x1: spans[k][1], text });
      [x0, text] = [spans[k][1], ''];
    }
  out.push({ x0, x1: e.x1, text });
  return out;
}

/** Spaces as one, ends trimmed. */
const tidy = (s: string) => s.trim().replace(/\s+/g, ' ');

/** One change to the field's text: `from[x0..x1]` becomes `text`. */
interface Edit {
  x0: number;
  x1: number;
  text: string;
}

/**
 * The stretches where `a` and `b` differ, as their index ranges `[i0, i1)` and `[j0, j1)`, from an alignment with the
 * fewest edits, where an item replaced costs one like an item put in or taken out (Levenshtein): a
 * replaced item stays in place, so the items around it stay unchanged. O(|a| × |b|): a page's words, or one Chinese or
 * Japanese run's characters (the field takes at most `FIX_MAX_CHARS` more than the page).
 */
function hunks<T>(a: T[], b: T[]): [number, number, number, number][] {
  // d[i][j]: the fewest edits that turn a[i..] into b[j..].
  const d = Array.from({ length: a.length + 1 }, () => new Array<number>(b.length + 1).fill(0));
  for (let i = a.length; i >= 0; i--)
    for (let j = b.length; j >= 0; j--)
      if (i === a.length || j === b.length) d[i][j] = a.length - i + b.length - j;
      else if (a[i] === b[j]) d[i][j] = d[i + 1][j + 1];
      else d[i][j] = 1 + Math.min(d[i + 1][j + 1], d[i + 1][j], d[i][j + 1]);
  const out: [number, number, number, number][] = [];
  let [i, j, i0, j0] = [0, 0, 0, 0];
  const close = () => {
    if (i > i0 || j > j0) out.push([i0, i, j0, j]);
  };
  while (i < a.length || j < b.length) {
    const both = i < a.length && j < b.length;
    if (both && a[i] === b[j]) {
      close();
      i++;
      j++;
      [i0, j0] = [i, j];
    } else if (both && d[i][j] === d[i + 1][j + 1] + 1) {
      // Of equally few edits: a replacement, else a removal, else an addition.
      i++;
      j++;
    } else if (i < a.length && (j === b.length || d[i][j] === d[i + 1][j] + 1)) i++;
    else j++;
  }
  close();
  return out;
}

/** The field's words as tokens between spaces: their text and where each sits. A CJK run is one token. */
function tokens(from: string): { text: string; at: number }[] {
  const out: { text: string; at: number }[] = [];
  let at = 0;
  for (const text of from.split(' ')) {
    if (text) out.push({ text, at });
    at += text.length + 1;
  }
  return out;
}

/**
 * The edits that turn `from` into `to`: a diff of their tokens (words between spaces), so letters added at a word's
 * edge stay in that word and two separate changes stay apart. A token changed into one token is diffed again by its
 * characters, which finds the words inside a Chinese or Japanese run. Tokens put in go after the token before (else
 * before the first), and tokens taken out go with the space before them (else after): an edit is never only a space,
 * and it never reaches into a word it doesn't change.
 */
function edits(from: string, to: string): Edit[] {
  const a = tokens(from);
  const b = to.split(' ');
  const end = (k: number) => a[k].at + a[k].text.length;
  const out: Edit[] = [];
  for (const [i0, i1, j0, j1] of hunks(
    a.map((t) => t.text),
    b,
  )) {
    const put = b.slice(j0, j1).join(' ');
    if (i0 === i1) {
      const x = i0 > 0 ? end(i0 - 1) : a[0].at;
      out.push({ x0: x, x1: x, text: i0 > 0 ? ` ${put}` : `${put} ` });
    } else if (j0 === j1)
      out.push(
        i0 > 0
          ? { x0: end(i0 - 1), x1: end(i1 - 1), text: '' }
          : { x0: a[0].at, x1: a[i1]?.at ?? from.length, text: '' },
      );
    else if (i1 - i0 === j1 - j0) {
      // As many tokens: each its own edit, by characters (a CJK run holds several words).
      for (let k = 0; k < i1 - i0; k++) {
        const [t, u] = [a[i0 + k], b[j0 + k]];
        for (const [c0, c1, d0, d1] of hunks([...t.text], [...u]))
          out.push({
            x0: t.at + offset(t.text, c0),
            x1: t.at + offset(t.text, c1),
            text: [...u].slice(d0, d1).join(''),
          });
      }
    } else out.push({ x0: a[i0].at, x1: end(i1 - 1), text: put });
  }
  return out;
}

/** The UTF-16 offset of code point `k` in `s` (a character diff walks code points, so an emoji stays whole). */
const offset = (s: string, k: number) => [...s].slice(0, k).join('').length;

/** The words an edit touches: those it overlaps, or for an insertion the word it is put into or after. */
function touched(spans: [number, number][], e: Edit): [number, number] {
  if (e.x1 > e.x0) {
    const first = spans.findIndex(([x, y]) => y > e.x0 && x < e.x1);
    let last = first;
    while (last + 1 < spans.length && spans[last + 1][0] < e.x1) last++;
    return [first, last];
  }
  let k = spans.findIndex(([x, y]) => x < e.x0 && e.x0 <= y);
  if (k < 0)
    k = Math.max(
      0,
      spans.findIndex(([x]) => x >= e.x0),
    );
  return [k, k];
}

/** Words `first..=last` to fix as one, and the edits inside them, in order. */
interface Stretch {
  first: number;
  last: number;
  edits: Edit[];
}

/** `stretches`, in order, with any two that share a word as one: two fixes never overlap. */
function merged(stretches: Stretch[]): Stretch[] {
  const out: Stretch[] = [];
  for (const s of stretches) {
    const prev = out.at(-1);
    if (prev && s.first <= prev.last) {
      prev.last = Math.max(prev.last, s.last);
      prev.edits.push(...s.edits);
    } else out.push(s);
  }
  return out;
}

/**
 * The fixes that turn the visible words of `all` (a page's words, or a run of them, in order; hidden ones included)
 * into `typed`, for captions in a style with `punct`: one per stretch of changed words, so every word the person didn't
 * change keeps its own reveal, keyword and line. A stretch's text is its words with the edits inside them made. A
 * stretch left empty (words taken out) takes a neighbour word in, so a fix is never blank; one the captions would draw
 * with a space next to it where none was typed, or the other way round, takes that neighbour in too (a Latin letter
 * typed inside a Chinese run, two Han words kept apart), so the spaces come out as typed. Stretches that come to share
 * a word are one. No stretch crosses a stop (`stops`): there an emptied stretch with no neighbour to take in is hidden
 * instead (`setWordLook`). None when nothing changed or nothing is left (taking words away is a cut, not a fix).
 */
export function fixOps(all: WordView[], typed: string, punct: CaptionPunctuation = 'keep'): Op[] {
  const words = visibleWords(all);
  const stop = stops(all);
  const { text: from, spans } = field(words);
  const to = tidy(typed);
  if (words.length === 0 || !to || to === from) return [];
  let stretches: Stretch[] = [];
  for (const e of edits(from, to).flatMap((e) => cutAtStops(e, spans, stop))) {
    const [first, last] = touched(spans, e);
    if (first >= 0) stretches.push({ first, last, edits: [e] });
  }
  /** A stretch's words with its edits made, a space an edit left at either edge kept. */
  const rawOf = (s: Stretch) => {
    const [lo, hi] = [Math.min(spans[s.first][0], ...s.edits.map((e) => e.x0)), spans[s.last][1]];
    let out = '';
    let at = lo;
    for (const e of s.edits) {
      out += from.slice(at, e.x0) + e.text;
      at = Math.max(at, e.x1);
    }
    return out + from.slice(at, Math.max(hi, at));
  };
  const textOf = (s: Stretch) => tidy(rawOf(s));
  /** What stands in word `k`'s place: its stretch's raw text, or the word. */
  const rawAt = (k: number) => {
    const s = stretches.find((t) => t.first <= k && k <= t.last);
    return s ? rawOf(s) : saidOf(words[k]);
  };
  /** An emptied stretch takes the word before it in, else the one after; never across a stop. */
  const fill = (s: Stretch) => {
    if (textOf(s)) return false;
    if (s.first > 0 && !stop[s.first - 1]) s.first--;
    else if (s.last < words.length - 1 && !stop[s.last]) s.last++;
    else return false;
    return true;
  };
  /**
   * Whether the captions would space `a` and `b` (raw texts side by side) otherwise than typed. Typed
   * joined: the field's flag, and no space an edit left on either side. Drawn joined: on what the style shows of the
   * two, as the pages judge it; a mark the style hides there spaces them the style's way, so it takes no word in.
   */
  const misdrawn = (a: string, b: string, flag: boolean) => {
    const typedJoined = flag && !/\s$/.test(a) && !/^\s/.test(b);
    const [ta, tb] = [tidy(a), tidy(b)];
    return !hidesMarkBetween(ta, tb, punct) && drawnJoined(ta, tb, punct) !== typedJoined;
  };
  /** A stretch the captions would draw spaced otherwise than typed from a neighbour takes that word in. */
  const grow = (s: Stretch) => {
    const raw = rawOf(s);
    if (s.first > 0 && !stop[s.first - 1] && misdrawn(rawAt(s.first - 1), raw, words[s.first].joined))
      s.first--;
    else if (
      s.last < words.length - 1 &&
      !stop[s.last] &&
      misdrawn(raw, rawAt(s.last + 1), words[s.last + 1].joined)
    )
      s.last++;
    else return false;
    return true;
  };
  // One word at a time, merged after each: stretches only grow, so this ends within a page's words of steps.
  stretches = merged(stretches);
  while (stretches.some(fill) || stretches.some(grow)) stretches = merged(stretches);
  const gone: WordView[] = [];
  const fixes = stretches.flatMap((s) => {
    const text = textOf(s);
    if (!text) gone.push(...words.slice(s.first, s.last + 1));
    const op = text ? fixOp(words.slice(s.first, s.last + 1), text) : null;
    return op ? [op] : [];
  });
  return [...fixes, ...lookOps(gone, { hidden: true })];
}
