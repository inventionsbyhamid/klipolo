import type { WordView } from '../../bindings/WordView';
import type { KeptHold } from '../../state/commitQueue';
import type { LookChange } from './captionOps';

/**
 * The look bar's presses and Spelling's saves made while the words are being updated, as plain functions: kept in
 * order, and once the words settle each is sent, built on the words then, or
 * dropped and said. A wait starts after the press that causes it and lasts less than a reaction, so a press made in
 * it is the person's, never one to throw away; but it goes only when nothing it was made for changed meanwhile.
 */

/** A look press, with the pick it was made on, the person's other actions so far, and the words lit. */
export interface QueuedLook {
  kind: 'look';
  change: LookChange;
  /** The selection's pick (`Sel.picked`, which stays as the selection follows its words). */
  picked: unknown;
  /** The person's other actions so far: typing in Spelling, undo and redo. */
  acts: number;
  lit: WordView[];
  /** The fixes this view had sent that the words lit show: one sent since makes them old. */
  fixes: number;
}

/** A Spelling save: the field's words, and what was typed over them (`from`: what they read before). */
export interface QueuedSpelling {
  kind: 'spelling';
  words: WordView[];
  text: string;
  from: string;
  /** Kept while the project closes or the app quits: the close waits for it (`keepAtClose`). */
  hold?: KeptHold;
}

export type Queued = QueuedLook | QueuedSpelling;

/** What the queue does now, in order. */
export type Step =
  | { kind: 'look'; change: LookChange; words: WordView[] }
  | { kind: 'spelling'; item: QueuedSpelling; words: WordView[] }
  | { kind: 'dropped' }
  | { kind: 'unsaved'; item: QueuedSpelling };

/** The selection once its words have settled, and where a spelling's words are now. */
export interface Settled {
  picked: unknown;
  acts: number;
  words: WordView[];
  /** The words found again in the settled answers, as one run, or null (moved apart, or not there). */
  find: (words: WordView[]) => WordView[] | null;
}

/** Each file's ranges in `words`, sorted by start (then end). */
function rangesOf(words: readonly WordView[]): Map<string, [number, number][]> {
  const byFile = new Map<string, [number, number][]>();
  for (const w of words) {
    const list = byFile.get(w.assetId);
    if (list) list.push([w.sourceStartUs, w.sourceEndUs]);
    else byFile.set(w.assetId, [[w.sourceStartUs, w.sourceEndUs]]);
  }
  for (const list of byFile.values()) list.sort((a, b) => a[0] - b[0] || a[1] - b[1]);
  return byFile;
}

/**
 * Whether every word of `now` holds only file words that were `lit`: it is a lit word or part of one (moved on the
 * timeline, or a correction undone), or a correction that starts and ends where lit words do and holds only lit words
 * between its ends (the person's own fix). A word moved in, another file's, or a correction that took in a word not lit
 * at its ends, isn't. An unlit word between two lit ones can't be told from a pause, since a `WordView` doesn't list a
 * correction's file words. A file's words never overlap (apart from copies, which share a range).
 * O((n + m) log m).
 */
export function within(lit: readonly WordView[], now: readonly WordView[]): boolean {
  const byFile = rangesOf(lit);
  return now.every((w) => {
    const list = byFile.get(w.assetId);
    if (!list) return false;
    const [s, e] = [w.sourceStartUs, w.sourceEndUs];
    // The first lit range starting at or after `s`.
    let [lo, hi] = [0, list.length];
    while (lo < hi) {
      const m = (lo + hi) >> 1;
      if (list[m][0] < s) lo = m + 1;
      else hi = m;
    }
    // Inside the lit word starting last at or before `s`.
    const before = lo < list.length && list[lo][0] === s ? lo : lo - 1;
    if (before >= 0) {
      let end = list[before][1];
      for (let k = before; k < list.length && list[k][0] === list[before][0]; k++)
        end = Math.max(end, list[k][1]);
      if (list[before][0] <= s && e <= end) return true;
    }
    // Or exactly over the lit words inside it.
    if (lo >= list.length || list[lo][0] !== s) return false;
    let end = s;
    for (let k = lo; k < list.length && list[k][0] < e; k++) {
      if (list[k][1] > e) return false;
      end = Math.max(end, list[k][1]);
    }
    return end === e;
  });
}

/**
 * Whether `a` is `b` as a spelling was typed over it: the same file words, reading the same, hidden or not alike, and
 * (but the first) cut from the word before alike. A ripple moves them alike.
 */
const asTyped = (a: WordView, b: WordView, k: number) =>
  a.assetId === b.assetId &&
  a.sourceStartUs === b.sourceStartUs &&
  a.sourceEndUs === b.sourceEndUs &&
  (a.look.hidden === true) === (b.look.hidden === true) &&
  (k === 0 || (a.cutBefore === true) === (b.cutBefore === true)) &&
  (a.said ?? a.text) === (b.said ?? b.text);

/** Whether `run` is the words `words` were, as their spelling was typed over them. O(n). */
export const sameSpelling = (run: readonly WordView[], words: readonly WordView[]) =>
  run.length === words.length && run.every((w, k) => asTyped(w, words[k], k));

/**
 * What goes now from the front of `queue`, the words having settled (`now`), in order, and what is left:
 * - a look press goes on the words selected now when they are the pick it was made on, the person did nothing else
 *   since (typed in Spelling, undid, redid), and every one was lit then (`within`); otherwise it is dropped;
 * - a spelling goes on its words found again (`find`) when they are still the words it was typed over
 *   (`sameSpelling`), whatever was picked since (leaving the field for another word saved it); else it is unsaved.
 *   What comes after a spelling sent waits for its words to change, and a spelling after a look sent waits for that
 *   look to land (a hide changes what a fix may cover).
 */
export function drain(queue: readonly Queued[], now: Settled): { steps: Step[]; rest: Queued[] } {
  const steps: Step[] = [];
  let k = 0;
  for (; k < queue.length; k++) {
    const q = queue[k];
    if (q.kind === 'look') {
      const same = q.picked !== null && q.picked === now.picked && q.acts === now.acts;
      const ok = same && now.words.length > 0 && within(q.lit, now.words);
      steps.push(ok ? { kind: 'look', change: q.change, words: now.words } : { kind: 'dropped' });
      continue;
    }
    if (steps.some((s) => s.kind === 'look')) break;
    const run = now.find(q.words);
    if (run && sameSpelling(run, q.words)) {
      steps.push({ kind: 'spelling', item: q, words: run });
      k++;
      break;
    }
    steps.push({ kind: 'unsaved', item: q });
  }
  return { steps, rest: queue.slice(k) };
}

/** What the queue does with what it keeps as the project closes, or the list does. */
export interface Settling {
  send: QueuedLook[];
  /** Look presses that can't go: said, in one toast. */
  lost: number;
  /** Spellings that can't wait: said. */
  unsaved: QueuedSpelling[];
  /** Spellings that wait for their pages while the project closes. */
  stay: QueuedSpelling[];
}

/**
 * What is kept, as the project closes (`closing`) or the list does:
 * - a look press goes now, on the words lit when it was pressed, when nothing it was made for changed since: the same
 *   pick, nothing else done, and no fix this view sent since the words it lit were drawn (one on its way, or landed
 *   with its pages still on theirs: the words it makes aren't known yet); otherwise it can't go;
 * - a spelling waits for its pages while the project closes (the close waits for it), and can't once the list has
 *   closed: nothing is left to send it.
 */
export function settleKept(
  queue: readonly Queued[],
  now: { picked: unknown; acts: number; fixes: number; closing: boolean },
): Settling {
  const out: Settling = { send: [], lost: 0, unsaved: [], stay: [] };
  for (const q of queue) {
    if (q.kind === 'spelling') (now.closing ? out.stay : out.unsaved).push(q);
    else if (q.picked !== null && q.picked === now.picked && q.acts === now.acts && q.fixes === now.fixes)
      out.send.push(q);
    else out.lost++;
  }
  return out;
}
