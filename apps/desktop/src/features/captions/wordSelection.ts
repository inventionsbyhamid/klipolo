import type { CaptionPagesView } from '../../bindings/CaptionPagesView';
import type { WordView } from '../../bindings/WordView';

/**
 * The word list's selection as plain functions
 * How a word is named, how a selection's words are found in an answer, and found again after an edit.
 */

/**
 * A word of the list by its file words (file, start and end in its file: a correction never shares its first word's
 * name) and its place on the timeline. It survives a refetch (a look leaves all of it as it is), and
 * The copies of a clip, which play the same file words, stay apart.
 */
export const wordKey = (w: WordView) => `${w.assetId}@${w.sourceStartUs}-${w.sourceEndUs}@${w.startUs}`;

/** A word's file words alone: the same in every copy, and through a ripple (Spelling's field is keyed by them). */
export const fileWordKey = (w: WordView) => `${w.assetId}@${w.sourceStartUs}-${w.sourceEndUs}`;

/** The most pages one answer holds (`captions_pages`' `MAX_PAGES`): a selection spans at most this many. */
export const MOST_PAGES = 200;

/** A word of an answer, with its page's row. */
export interface Listed {
  key: string;
  word: WordView;
  row: number;
}

/**
 * The anchor and the focus of a selection, each by key and by its page's row, and the words the person picked for
 * them, kept as picked: what the selection falls back on when the words it holds can't be found (a pick an edit
 * overtook, or undo of a correction it followed into).
 */
export interface Sel {
  anchor: string;
  anchorRow: number;
  focus: string;
  focusRow: number;
  picked: [WordView, WordView];
}

/** An answer's words in order, each with its row. */
export function listedOf(view: CaptionPagesView | null): Listed[] {
  const first = view?.first ?? 0;
  return (view?.pages ?? []).flatMap((page, i) =>
    page.words.map((word) => ({ key: wordKey(word), word, row: first + i })),
  );
}

/**
 * An answer's words, indexed once: by key, and per file sorted by where they start in it, so a word
 * is found again by a binary search over its file. O(n log n) to make, beside `listed`.
 */
export interface Index {
  at: Map<string, number>;
  /** Per file, the indexes into `listed`, sorted by file start, then by timeline start. */
  byFile: Map<string, number[]>;
}

export function indexOf(listed: Listed[]): Index {
  const at = new Map<string, number>();
  const byFile = new Map<string, number[]>();
  const [start, heard] = [new Float64Array(listed.length), new Float64Array(listed.length)];
  listed.forEach((l, i) => {
    at.set(l.key, i);
    [start[i], heard[i]] = [l.word.sourceStartUs, l.word.startUs];
    const list = byFile.get(l.word.assetId);
    if (list) list.push(i);
    else byFile.set(l.word.assetId, [i]);
  });
  const before = (a: number, b: number) => start[a] - start[b] || heard[a] - heard[b];
  // Most of an answer plays its file in order: sorted only when it doesn't.
  for (const list of byFile.values())
    if (list.some((i, k) => k > 0 && before(list[k - 1], i) > 0)) list.sort(before);
  return { at, byFile };
}

/** The middle of a word in its file. */
const mid = (w: WordView) => w.sourceStartUs + (w.sourceEndUs - w.sourceStartUs) / 2;

/**
 * Where `old`'s file word is in `listed` now: the same key, else a word of the same file whose range holds `old`'s
 * middle (itself, moved on the timeline, or the correction it is now part of), the copy nearest `old`'s timeline
 * start. -1 when none. O(log n + copies): a file's page words never overlap, apart from copies, which share a range.
 */
export function refind(listed: Listed[], index: Index, old: WordView, key = wordKey(old)): number {
  const same = index.at.get(key);
  if (same !== undefined) return same;
  const list = index.byFile.get(old.assetId);
  if (!list) return -1;
  const middle = mid(old);
  // The last word starting at or before the middle; those starting there too are its copies, just before it.
  let [lo, hi] = [0, list.length];
  while (lo < hi) {
    const m = (lo + hi) >> 1;
    if (listed[list[m]].word.sourceStartUs <= middle) lo = m + 1;
    else hi = m;
  }
  if (lo === 0) return -1;
  const start = listed[list[lo - 1]].word.sourceStartUs;
  let [best, gap] = [-1, Infinity];
  for (let k = lo - 1; k >= 0 && listed[list[k]].word.sourceStartUs === start; k--) {
    const w = listed[list[k]].word;
    const d = Math.abs(w.startUs - old.startUs);
    if (middle < w.sourceEndUs && d < gap) [best, gap] = [list[k], d];
  }
  return best;
}

/**
 * The words next to `listed[i]` that `old`, a correction undone, was made of: those of its file next
 * on the timeline, each on in the file from the one before and with its middle inside `old`'s range: `[first, last]`. A
 * word that was no correction is just `[i, i]`, and so is a copy played right after it (it starts in the file where the
 * word does). O(the words it was made of).
 */
function madeOf(listed: Listed[], i: number, old: WordView): [number, number] {
  const inside = (w: WordView) =>
    w.assetId === old.assetId && mid(w) >= old.sourceStartUs && mid(w) < old.sourceEndUs;
  let [a, b] = [i, i];
  while (
    a > 0 &&
    inside(listed[a - 1].word) &&
    listed[a - 1].word.sourceEndUs <= listed[a].word.sourceStartUs
  )
    a--;
  while (
    b + 1 < listed.length &&
    inside(listed[b + 1].word) &&
    listed[b + 1].word.sourceStartUs >= listed[b].word.sourceEndUs
  )
    b++;
  return [a, b];
}

/**
 * Where the words `old` (a selection's, in order) are in `listed` now: `[first, last]` when each is found again
 * (`refind`) and together they are still one run of words; `apart` when they are all there but other words came
 * between them (a section moved away); `missing` as soon as one isn't in `listed` (gone, or on pages it doesn't hold).
 * A correction undone stands for the words it was made of (`madeOf`), and when it is an end of the run, the run stops
 * at the word picked there (`ends`: the person's picks at `old`'s first and last word) if it is one of them: undo gives
 * back the words selected before the correction. O(m log n).
 */
export function follow(
  listed: Listed[],
  index: Index,
  old: Listed[],
  ends?: readonly [WordView, WordView],
): [number, number] | 'apart' | 'missing' {
  if (old.length === 0) return 'missing';
  // The words they are now, each once (several old words can be one correction now, one old word several words).
  const seen = new Uint8Array(listed.length);
  let [lo, hi, count] = [Infinity, -1, 0];
  let first: [number, number] | null = null;
  let last: [number, number] = [-1, -1];
  for (const o of old) {
    const i = refind(listed, index, o.word, o.key);
    if (i < 0) return 'missing';
    const span = listed[i].key === o.key ? ([i, i] as [number, number]) : madeOf(listed, i, o.word);
    for (let k = span[0]; k <= span[1]; k++) if (!seen[k]) [seen[k], count] = [1, count + 1];
    [lo, hi] = [Math.min(lo, span[0]), Math.max(hi, span[1])];
    first ??= span;
    last = span;
  }
  if (hi - lo + 1 !== count) return 'apart';
  if (ends && first) {
    const [p, q] = [refind(listed, index, ends[0]), refind(listed, index, ends[1])];
    const [a, b] = [first, last];
    const inA = (k: number) => k >= a[0] && k <= a[1];
    const inB = (k: number) => k >= b[0] && k <= b[1];
    // One correction undone: the words picked in it, in either order; else each end at its own pick.
    if (old.length === 1) {
      if (inA(p) && inA(q)) [lo, hi] = [Math.min(p, q), Math.max(p, q)];
    } else {
      if (a[0] === lo && inA(p)) lo = p;
      if (b[1] === hi && inB(q)) hi = q;
    }
  }
  return [lo, hi];
}

/** An answer's words and their index. */
export type Answer = readonly [Listed[], Index];

/** A selection and the words it held, as the last render found them (`sel` null: none yet). */
export interface Held {
  sel: Sel | null;
  chosen: Listed[];
}

/** Whether every word of `l[from..=to]` is one of `old`'s words, or a word the person picked, found again. */
function onlyOf(l: Listed[], index: Index, from: number, to: number, old: Listed[], picked: Sel['picked']) {
  const hits = new Set([
    ...old.map((o) => refind(l, index, o.word, o.key)),
    ...picked.map((w) => refind(l, index, w)),
  ]);
  for (let k = from; k <= to; k++) if (!hits.has(k)) return false;
  return true;
}

/**
 * Where `sel`'s words are after an edit, in `answers` (the window's, then the selection's own):
 * - while no pick came since `was` was found (`same`), the words it held (`follow`, its picks at the ends), in the
 *   first answer that has them all; it lets go (null) when a section of them moved away;
 * - else, or when they aren't all there, the run between the words the person picked, found again in the answer where
 *   they lie nearest where they were picked (a copy can be in the other): a pick an edit overtook, or words cut from
 *   from inside it (an "um"). Unless a pick came since, it lets go when that run holds words it didn't;
 * - `missing` when the picks aren't in these answers either.
 */
export function findAgain(
  answers: readonly Answer[],
  sel: Sel,
  was: Held,
): { run: Listed[]; anchorFirst: boolean } | null | 'missing' {
  const same = was.sel !== null && was.sel.anchor === sel.anchor && was.sel.focus === sel.focus;
  if (same) {
    const anchorFirst = was.chosen[0]?.key === sel.anchor;
    const ends = anchorFirst ? sel.picked : ([sel.picked[1], sel.picked[0]] as const);
    for (const [l, ix] of answers) {
      const found = follow(l, ix, was.chosen, ends);
      if (found === 'apart') return null;
      if (found !== 'missing') return { run: l.slice(found[0], found[1] + 1), anchorFirst };
    }
  }
  let best: { l: Listed[]; ix: Index; i: number; j: number; gap: number } | null = null;
  for (const [l, ix] of answers) {
    const [i, j] = [refind(l, ix, sel.picked[0]), refind(l, ix, sel.picked[1])];
    if (i < 0 || j < 0) continue;
    const gap =
      Math.abs(l[i].word.startUs - sel.picked[0].startUs) +
      Math.abs(l[j].word.startUs - sel.picked[1].startUs);
    if (!best || gap < best.gap) best = { l, ix, i, j, gap };
  }
  if (!best) return 'missing';
  const { l, ix, i, j } = best;
  const [a, b] = [Math.min(i, j), Math.max(i, j)];
  if (same && !onlyOf(l, ix, a, b, was.chosen, sel.picked)) return null;
  return { run: l.slice(a, b + 1), anchorFirst: i <= j };
}

/** The selection over `run` (in order), its anchor at the end the old one's was, the person's picks kept. */
export function selOver(run: Listed[], anchorFirst: boolean, picked: Sel['picked']): Sel {
  const [a, b] = anchorFirst ? [run[0], run[run.length - 1]] : [run[run.length - 1], run[0]];
  return { anchor: a.key, anchorRow: a.row, focus: b.key, focusRow: b.row, picked };
}

/** Whether `next` names the same words and rows as `sel`. */
export const sameSel = (sel: Sel, next: Sel) =>
  sel.anchor === next.anchor &&
  sel.focus === next.focus &&
  sel.anchorRow === next.anchorRow &&
  sel.focusRow === next.focusRow;

/** Whether `run` holds exactly the words `old` holds, in order (by key). */
export const sameWords = (run: Listed[], old: Listed[]) =>
  run.length === old.length && run.every((l, k) => l.key === old[k].key);
