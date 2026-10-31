import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { CaptionPagesQuery } from '../../bindings/CaptionPagesQuery';
import type { CaptionPagesView } from '../../bindings/CaptionPagesView';
import type { FormatKind } from '../../bindings/FormatKind';
import type { WordView } from '../../bindings/WordView';
import { useCaptionAnswer } from './useCaptionPages';
import {
  findAgain,
  follow,
  indexOf,
  listedOf,
  MOST_PAGES,
  sameSel,
  sameWords,
  selOver,
  wordKey,
  type Answer,
  type Held,
  type Listed,
  type Sel,
} from './wordSelection';

export { fileWordKey, MOST_PAGES, wordKey, type Listed } from './wordSelection';

/** Pages asked on each side of a selection's own rows, so words a change moved to a page nearby are found. */
const PAD = 10;

/** Why the selection asks an answer of its own though it lies in the window: around its rows, then by its time. */
type Lost = null | 'rows' | { fromUs: number; toUs: number };

/** What one render makes of the selection. */
interface Resolved {
  chosen: Listed[];
  /** The selection those words are of: an earlier one while its own answer is on its way. */
  of: Sel | null;
  /** The selection to keep instead: its words at new rows or keys, or null when they are gone. */
  next?: Sel | null;
  /** Its words aren't in the answers it has: ask its own (`Lost`) before giving up. */
  lost?: Lost;
  /** The revision the answer its words come from was asked at (null: not yet known). */
  base: string | null;
}

/**
 * The word list's selection: an anchor and a focus, kept by key and page row, and the words the person picked for
 * picked for them.
 * - Its words come from the window's answer while they lie in it, else from an answer of its own (its rows and up to
 *   `PAD` pages each side, at most `MOST_PAGES`), so it never shrinks as the list scrolls.
 * - The words it had stay only while that answer of its own is on its way. It is `pending` while the words it holds
 *   are from an earlier revision than the project's now (`viewOf`: the window's answer's), or about to change: the
 *   look bar keeps its presses until then ("Updating…"), so none acts on words an edit has moved. Scrolling away from
 *   it asks its own answer again, but its words are still the revision's: no wait.
 * - After an edit, each of its words is found again by its file word (`findAgain`, indexed). It
 *   follows them while they are still one run (into a correction, and back to the words picked when it is undone)
 *   and lets go when a section moved away; the same two ends with other words between them are never taken as
 *   they are. When some can't be found it falls back on the words the person picked: a pick an edit overtook
 *, or words cut from inside it. Words not in the answers it has are asked for around its rows, then by the time
 *   time), before it lets go.
 * - A selection that would span more pages isn't extended, and says so (`tooLong`).
 * - `pickRow` picks a whole page's words, once the window's answer holds them (opened from the video).
 */
export function useWordSelection(format: FormatKind, view: CaptionPagesView | null, viewOf: string | null) {
  const [sel, setSel] = useState<Sel | null>(null);
  const [lost, setLost] = useState<Lost>(null);
  const [tooLong, setTooLong] = useState(false);
  const listed = useMemo(() => listedOf(view), [view]);
  const index = useMemo(() => indexOf(listed), [listed]);
  const [lo, hi] = sel
    ? [Math.min(sel.anchorRow, sel.focusRow), Math.max(sel.anchorRow, sel.focusRow)]
    : [0, -1];
  const inView = view !== null && lo >= view.first && hi < view.first + view.pages.length;
  const pad = Math.max(0, Math.min(PAD, Math.floor((MOST_PAGES - (hi - lo + 1)) / 2)));
  const from = Math.max(0, lo - pad);
  const asked = sel !== null && (!inView || lost !== null);
  let query: CaptionPagesQuery | null = null;
  if (asked)
    query =
      lost !== null && lost !== 'rows'
        ? { type: 'range', fromUs: lost.fromUs, toUs: lost.toUs }
        : { type: 'index', from, count: hi + pad + 1 - from };
  const { view: own, current, base } = useCaptionAnswer(format, query);
  const ownListed = useMemo(() => (current ? listedOf(own) : []), [own, current]);
  const ownIndex = useMemo(() => indexOf(ownListed), [ownListed]);
  /** The selection the words it holds were resolved for, those words, and the revision they are of. */
  const kept = useRef<Held & { base: string | null }>({ sel: null, chosen: [], base: null });
  const resolved = useMemo<Resolved>(() => {
    if (!sel) return { chosen: [], of: null, base };
    const was = kept.current;
    // No pick since its words were found: they are this selection's.
    const same = was.sel !== null && was.sel.anchor === sel.anchor && was.sel.focus === sel.focus;
    const answers: Answer[] = [
      [listed, index],
      [ownListed, ownIndex],
    ];
    for (const [l, ix] of answers) {
      const of = l === listed ? viewOf : base;
      const [i, j] = [ix.at.get(sel.anchor), ix.at.get(sel.focus)];
      if (i === undefined || j === undefined) continue;
      const run = l.slice(Math.min(i, j), Math.max(i, j) + 1);
      // Its two ends with other words between them now (two edits landed as one): found again instead.
      if (same && !sameWords(run, was.chosen)) break;
      // Its rows as the answer has them now (a change can move its words to other pages).
      const next = { ...sel, anchorRow: l[i].row, focusRow: l[j].row };
      return { chosen: run, of: sel, next: sameSel(sel, next) ? undefined : next, base: of };
    }
    // Its own answer is on its way: the words it had stay until it is in (still those of the selection they were of:
    // after a pick, the one before, so they are behind it as words of an earlier revision are).
    if (asked && !current) return { chosen: was.chosen, of: was.sel, base: same ? was.base : null };
    // After an edit: its words, or the words picked, found again (`findAgain`), or let go.
    const found = findAgain(answers, sel, was);
    if (found === null) return { chosen: [], of: null, next: null, base };
    if (found !== 'missing') {
      const next = selOver(found.run, found.anchorFirst, sel.picked);
      return { chosen: found.run, of: next, next, base: null };
    }
    // Not in the answers it has: its own, around its rows, then by the time its words play, before letting go.
    if (!asked) return { chosen: was.chosen, of: was.sel, lost: 'rows', base: null };
    const [first, last] = [was.chosen[0], was.chosen[was.chosen.length - 1]];
    if (first && (lost === null || lost === 'rows'))
      return {
        chosen: was.chosen,
        of: was.sel,
        lost: { fromUs: first.word.startUs, toUs: last.word.endUs },
        base: null,
      };
    return { chosen: [], of: null, next: null, base };
  }, [sel, listed, index, ownListed, ownIndex, asked, current, lost, viewOf, base]);
  kept.current = { sel: resolved.of, chosen: resolved.chosen, base: resolved.base };
  useEffect(() => {
    if (resolved.next !== undefined) {
      setSel(resolved.next);
      setLost(null);
    }
    if (resolved.next === null) setTooLong(false);
    if (resolved.lost !== undefined) setLost(resolved.lost);
  }, [resolved]);
  const words = useMemo(() => resolved.chosen.map((l) => l.word), [resolved]);
  const selected = useMemo(() => new Set(resolved.chosen.map((l) => l.key)), [resolved]);
  // Where the arrow keys go from: the answer that holds the active word, or none while it is asked.
  const nav = !sel
    ? null
    : index.at.has(sel.focus)
      ? { list: listed, at: index.at }
      : ownIndex.at.has(sel.focus)
        ? { list: ownListed, at: ownIndex.at }
        : null;
  // Also while it is about to change (its words at new rows, or asked for again): no "ready" between two round trips.
  const pending =
    sel !== null && (resolved.base !== base || resolved.next !== undefined || resolved.lost !== undefined);
  /**
   * `words` (a spelling's) found again in the answers at the revision now, as one run (`follow`), or null: moved
   * apart, or not in them. O(m log n).
   */
  const find = (ws: WordView[]): WordView[] | null => {
    const old = ws.map((word) => ({ key: wordKey(word), word, row: 0 }));
    const answers: Answer[] = viewOf === base ? [[listed, index]] : [];
    if (current) answers.push([ownListed, ownIndex]);
    for (const [l, ix] of answers) {
      const found = follow(l, ix, old);
      if (found === 'apart') return null;
      if (found !== 'missing') return l.slice(found[0], found[1] + 1).map((x) => x.word);
    }
    return null;
  };

  /** Picks `l`, or extends the selection to it, unless that would span more than `MOST_PAGES` pages. */
  const pick = (l: Listed, extend: boolean) => {
    if (extend && sel) {
      if (Math.abs(l.row - sel.anchorRow) >= MOST_PAGES) {
        setTooLong(true);
        return;
      }
      setSel({ ...sel, focus: l.key, focusRow: l.row, picked: [sel.picked[0], l.word] });
    } else
      setSel({ anchor: l.key, anchorRow: l.row, focus: l.key, focusRow: l.row, picked: [l.word, l.word] });
    setTooLong(false);
    setLost(null);
  };
  const clear = () => {
    setSel(null);
    setTooLong(false);
    setLost(null);
  };
  /** Picks every word of the page at `row` (first to last), if the window's answer holds them: whether it could. */
  const pickRow = useCallback(
    (row: number) => {
      const run = listed.filter((l) => l.row === row);
      if (run.length === 0) return false;
      setSel(selOver(run, true, [run[0].word, run[run.length - 1].word]));
      setTooLong(false);
      setLost(null);
      return true;
    },
    [listed],
  );
  return {
    sel,
    picked: sel?.picked ?? null,
    // The pick the words it holds were found for: after a new pick, the one before until its words are in.
    heldPick: resolved.of?.picked ?? null,
    listed,
    nav,
    words,
    selected,
    tooLong,
    pending,
    find,
    pick,
    pickRow,
    clear,
  };
}
