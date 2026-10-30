/**
 * The timeline's edits (drag commits, a held nudge, the keyboard's edits) send one commit at a time. Rust accepts an
 * edit built before this window's own previous edit landed (`useProject.apply`), so an edit built while another is
 * landing would build on the project that one replaces: a nudge pressed during a drop's commit would put the layer
 * back. A commit (or a chat change being merged, `useProject.adopt`) holds the queue from `beginCommit` to the `end`
 * it returns; work that builds on the project waits with `afterCommits`, and runs from the project it left.
 */
import type { Label } from '../lib/labels';

let inFlight = 0;
/**
 * Work waiting its turn. `kept`: a change kept at close (`keepAtClose`), which `abandonEdits` keeps; `edit`: false for
 * a wait that makes no edit itself (a drain, a drag's turn), which the close doesn't count as an edit it dropped.
 */
const waiting: { run: () => void; kept: KeptHold | null; edit: boolean }[] = [];
/** What `onAbandon` registered: each says how many edits it dropped, if any. */
const droppers = new Set<() => number | void>();
/** What `onCloseStart` registered. */
const stoppers = new Set<() => void>();

/**
 * A change closing waits for rather than drops (`keepAtClose`): a burst's or a caption change waiting its turn (one
 * carries a whole value, such as the caption settings, so they go in order, each built once the one before has
 * landed), or a word list's spelling waiting for its pages. `release` once it has gone in or been said; `dropped` once
 * the close gave up on it.
 */
export interface KeptHold {
  /** Its undo label: what is said if the close can't wait for it (set again as it goes, when worded then). */
  label: Label;
  release(): void;
  dropped: boolean;
  /** Its commit has begun: it goes ahead of the close in any case, so the close neither says it nor drops it. */
  started: boolean;
  /** What a commit waiting its turn does when the close drops it: its caller hears it didn't go in. */
  drop?: () => void;
}
const kept = new Set<KeptHold>();
const keptWaiters: (() => void)[] = [];

/** Holds the close until `release` (at most `CLOSE_WAIT_MS`, in `landEdits`), and quitting until then too. */
export function keepAtClose(label: Label): KeptHold {
  const hold: KeptHold = {
    label,
    dropped: false,
    started: false,
    release: () => {
      if (!kept.delete(hold) || kept.size > 0) return;
      for (const resolve of keptWaiters.splice(0)) resolve();
    },
  };
  kept.add(hold);
  return hold;
}

/** Resolves once every change kept at close has gone in or been said (at once when there are none). */
export function keptDone(): Promise<void> {
  if (kept.size === 0) return Promise.resolve();
  return new Promise((resolve) => keptWaiters.push(resolve));
}

/** The changes kept at close that haven't begun to go in yet, oldest first (one that has will land first). */
export function unsavedKept(): Label[] {
  return [...kept].filter((hold) => !hold.started).map((hold) => hold.label);
}

/**
 * The close can't wait any longer: what is kept and hasn't begun is dropped (it was said); a commit waiting its turn
 * hears it didn't go in (`drop`), so nothing waits on it. `keptDone` resolves.
 */
export function dropKept() {
  for (let k = waiting.length - 1; k >= 0; k--) {
    const hold = waiting[k].kept;
    if (hold && !hold.started) waiting.splice(k, 1);
  }
  for (const hold of kept) {
    if (hold.started) continue;
    hold.dropped = true;
    kept.delete(hold);
    hold.drop?.();
  }
  for (const resolve of keptWaiters.splice(0)) resolve();
}

export function commitInFlight(): boolean {
  return inFlight > 0;
}

/** A commit is on its way; call the returned `end` once it has settled (landed or failed). */
export function beginCommit(): () => void {
  inFlight++;
  let ended = false;
  return () => {
    if (ended) return;
    ended = true;
    inFlight--;
    // Each in turn: one that sends a commit holds the rest until that lands.
    while (inFlight === 0 && waiting.length > 0) {
      try {
        waiting.shift()!.run();
      } catch (error) {
        console.error(error);
      }
    }
  };
}

/**
 * Runs `action` once no commit is in flight: now when none is. `first`: ahead of everything already waiting, for work
 * that had its turn and gave it up to let a burst go in first (it still goes before what waited after it). `kept`: a
 * change kept at close (`keepAtClose`), said by its label if the close can't wait for it. `edit: false`: a wait that
 * makes no edit itself (a drain, a drag's turn, whose drops say their own count). Returns a function that runs it at
 * once if it is still waiting (nothing uses that at close any more: a change goes in its turn).
 */
export function afterCommits(
  action: () => void,
  {
    first = false,
    kept: hold = null,
    edit = true,
  }: { first?: boolean; kept?: KeptHold | null; edit?: boolean } = {},
): () => void {
  let ran = false;
  const once = () => {
    if (ran) return;
    ran = true;
    action();
  };
  const entry = { run: once, kept: hold, edit };
  if (inFlight === 0) once();
  else if (first) waiting.unshift(entry);
  else waiting.push(entry);
  return once;
}

/** A commit is on its way, or work waits its turn. */
export function commitsPending(): boolean {
  return inFlight > 0 || waiting.length > 0;
}

/** Sends the keyboard's change still settling (`keyBurst`'s held nudge); true when that put a commit in flight. */
let settling: () => boolean = () => false;

/** `keyBurst` registers its flush, for `yieldToSettling`. */
export function onSettling(flush: () => boolean) {
  settling = flush;
}

/**
 * For work at its turn that builds on every edit made before it (undo, redo, an export): a nudge that a press waiting
 * ahead of it left settling goes in first, and `run` then goes again before anything that waited after it
 * it gave up its turn. (`keyBurst.after` and the drag queue do the same for their own waiters.)
 * `edit: false`: `run` makes no edit itself (a drain).
 */
export function yieldToSettling(run: () => void, { edit = true }: { edit?: boolean } = {}): boolean {
  if (!settling()) return false;
  afterCommits(run, { first: true, edit });
  return true;
}

/**
 * Resolves once every commit in flight, and every edit waiting its turn, has gone in (at once when there are none):
 * true, or false when the project closes first. Export waits on it, so a slider released just before ⌘E is exported;
 * so do quitting and closing (`landEdits`), so no edit made before them is lost.
 */
export function commitsDone(): Promise<boolean> {
  return new Promise((resolve) => {
    const unregister = onAbandon(() => {
      unregister();
      resolve(false);
    });
    const done = () => {
      if (yieldToSettling(done, { edit: false })) return;
      unregister();
      resolve(true);
    };
    afterCommits(done, { edit: false });
  });
}

/**
 * The project closes once it has waited for the edits made before it (`landEdits`): what still waits to build is
 * dropped (past the close's bound, or asked for since), and so is what `onAbandon` registered; a change kept at close
 * (`keepAtClose`) is left for `keptDone` and `dropKept`. Returns how many edits it dropped that have no label to say
 * them by (a timeline edit, an undo, a queued drop), for the close to count.
 */
export function abandonEdits(): number {
  let edits = 0;
  for (let k = waiting.length - 1; k >= 0; k--) {
    const entry = waiting[k];
    if (entry.kept) continue;
    waiting.splice(k, 1);
    if (entry.edit) edits++;
  }
  for (const drop of droppers) {
    const dropped = drop();
    if (typeof dropped === 'number') edits += dropped;
  }
  return edits;
}

/**
 * `drop` runs when the project closes (a drag's drops still queued), returning how many edits it dropped, if any.
 * Returns the unregister function.
 */
export function onAbandon(drop: () => number | void): () => void {
  droppers.add(drop);
  return () => {
    droppers.delete(drop);
  };
}

/**
 * `stop` runs as the project starts to close, before it waits for the edits made (a drag under way ends: it is not an
 * edit yet). Returns the unregister function.
 */
export function onCloseStart(stop: () => void): () => void {
  stoppers.add(stop);
  return () => {
    stoppers.delete(stop);
  };
}

/** The project starts to close: what `onCloseStart` registered stops; edits waiting their turn stay for theirs. */
export function closeStarts() {
  for (const stop of stoppers) stop();
}
