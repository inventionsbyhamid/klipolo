/**
 * Toolbar changes still settling into one undo step (`useBurst`: − and + presses, slider arrow keys, a custom colour
 * being picked). Undo and redo commit them first, so ⌘Z undoes the change just made rather than the one before it.
 */
const flushes = new Set<(closing: boolean) => boolean>();

/** How long a burst waits after its last change before it commits (ms): presses within it are one undo step. */
export const SETTLE_MS = 400;

/**
 * `flush` commits its pending change and says whether there was one; `closing`, the project is closing (or the app
 * quitting). A burst's commit is kept at close (`useBurst`): it goes in its turn, and the close waits for it. Returns
 * the unregister function.
 */
export function registerBurst(flush: (closing: boolean) => boolean): () => void {
  flushes.add(flush);
  return () => {
    flushes.delete(flush);
  };
}

/**
 * Closing's rounds at most. Today's deepest nesting takes two to three: a word colour's key settle presses into the
 * word list's queue while its words update, the queue's settle sends it, kept at close; a colour's key
 * settle pushed into its field's burst goes in the next round. Six leave room; a round with nothing pending ends it.
 */
const CLOSING_ROUNDS = 6;

/**
 * Commits every pending burst now (through the commit queue, so undo, redo and export, which wait their turn after
 * it, come after it); true if any was pending. `closing`: each goes into the commit queue now, kept at close, so the
 * close waits for it in its turn (`landEdits`: never at once, ahead of a commit it waits for, which it would put
 * back). A burst can commit through another (a colour's key settle through its field's `useBurst`) that this
 * round has already passed: closing goes round again until a round finds nothing pending (a burst with nothing pending
 * does nothing and says false).
 */
export function flushBursts({ closing = false }: { closing?: boolean } = {}): boolean {
  let flushed = false;
  for (let round = 0; round < CLOSING_ROUNDS; round++) {
    let any = false;
    for (const flush of flushes) any = flush(closing) || any;
    flushed ||= any;
    if (!closing || !any) break;
  }
  return flushed;
}
