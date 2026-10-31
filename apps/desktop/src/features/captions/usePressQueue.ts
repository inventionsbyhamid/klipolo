import { plural } from '@lingui/core/macro';
import { useLingui } from '@lingui/react/macro';
import { useEffect, useRef, useState } from 'react';
import type { CaptionLook } from '../../bindings/CaptionLook';
import type { Op } from '../../bindings/Op';
import type { WordView } from '../../bindings/WordView';
import { useStableCallback } from '../../lib/useStableCallback';
import { registerBurst } from '../../state/bursts';
import { keepAtClose } from '../../state/commitQueue';
import { historySteps } from '../../state/project';
import { toast } from '../../state/toasts';
import { useBurst } from '../preview/useBurst';
import { announce } from '../timeline/announce';
import { captionPunctuation, currentLook, lookOps, withChange, type LookChange } from './captionOps';
import { drain, settleKept, type Queued, type QueuedSpelling } from './pressQueue';
import { fileWordKey } from './wordSelection';
import { fixOps } from './wordFix';

/** A look change's undo label: hiding and showing words say so; any other look is "Style caption words". */
const lookLabel = (change: LookChange) =>
  typeof change === 'object' && 'hidden' in change
    ? change.hidden
      ? ('captions.hide.undo' as const)
      : ('captions.show.undo' as const)
    : ('captions.look.undo' as const);

/** `ws` with each word's look as the project holds it now (the look bar's dry run builds on it too). */
export const lookedNow = (ws: WordView[]) => ws.map((w) => ({ ...w, look: currentLook(w) }));
const fieldKey = (ws: WordView[]) => ws.map(fileWordKey).join(' ');
/** A spelling's undo label (its id, with the words it fixes and the words it makes). */
const fixLabel = (item: QueuedSpelling) => ({
  id: 'captions.fix.undo' as const,
  params: { from: item.from, to: item.text.trim() },
});

/**
 * The look bar's presses and Spelling's saves. While the words are being updated
 * (`pending`: an edit's pages on their way), or a fix this view sent is, they are `busy`: a press or a save is kept, in
 * order (`pressQueue`), not dropped. Once every answer is in at the revision now and what this view sent has landed,
 * each goes, built on the words then, or is dropped and said on the "Updating…" line ("The words changed. Press
 * again."; a spelling: that it wasn't saved). Otherwise a press or a save goes at once on the words pressed, each
 * built as it goes in, from the looks then (and those of this view's presses on their way), through the
 * bar's burst: one still waiting its turn in the commit queue when the project closes goes in its turn, the close
 * waiting for it, the word colour that the close commits from its keys included, and so
 * does a spelling. What is kept
 * when the project or the list closes goes too, waits for its pages (a spelling, while the project closes), or is said
 * with a toast (`settle`): never lost silently, a press that comes after the list closed included.
 */
export function usePressQueue({
  words,
  picked,
  heldPick,
  pending,
  fresh,
  find,
}: {
  words: WordView[];
  /** The selection's pick, or null; and the one the words it holds were found for. */
  picked: unknown;
  heldPick: unknown;
  /** The selection's words are behind the revision, or about to change. */
  pending: boolean;
  /** The window's answer is at the revision now. */
  fresh: boolean;
  find: (words: WordView[]) => WordView[] | null;
}) {
  const { t } = useLingui();
  const burst = useBurst();
  const queue = useRef<Queued[]>([]);
  const [queued, setQueued] = useState(0);
  /** Commits this view sent that haven't landed, and how many of them are fixes (which change the words). */
  const [sent, setSent] = useState({ all: 0, fixes: 0 });
  const fixing = useRef(0);
  const typing = useRef(0);
  /** Fixes this view sent; and how many of them the words drawn show (settled, none on its way). */
  const sentFixes = useRef(0);
  const drawnFixes = useRef(0);
  if (!pending && fresh && fixing.current === 0) drawnFixes.current = sentFixes.current;
  /**
   * The list has closed: a press after it (a colour's key settle flushing as it unmounts, after this hook's
   * cleanup) goes, or is said, at once; what goes is kept at close like every burst's commit.
   */
  const gone = useRef(false);
  /**
   * The looks this view's look commits give their words (by file word) from when each is built until it lands: a press
   * built before the one ahead of it lands builds on it, never under it (each now waits its turn, at close too: this
   * stays as a guard for a commit under way).
   */
  const ahead = useRef(new Map<string, { look: CaptionLook; by: number }>());
  const sends = useRef(0);
  const lookedAhead = (ws: WordView[]) =>
    ws.map((w) => ({ ...w, look: ahead.current.get(fileWordKey(w))?.look ?? currentLook(w) }));
  const [note, setNote] = useState<string | null>(null);
  const busy = pending || sent.fixes > 0 || queued > 0;
  /** What a press sees: this render's selection, and what was sent or kept since it was drawn. */
  const live = useRef({ pending, words, picked, heldPick });
  live.current = { pending, words, picked, heldPick };
  const busyNow = () => live.current.pending || fixing.current > 0 || queue.current.length > 0;
  const acts = () => typing.current + historySteps();
  const keep = (next: Queued[]) => {
    queue.current = next;
    setQueued(next.length);
  };
  /**
   * Shows `text` on the count's line and says it in the editor's live region: a note that mounts with its text isn't
   * reliably read by VoiceOver, and these are the only sign a press or a save didn't go.
   */
  const tell = (text: string) => {
    setNote(text);
    announce(text);
  };

  /** Spellings said not to be saved, by field and text: Enter in that field saves one again. */
  const saidTexts = useRef(new Set<string>());
  const textKey = (item: { words: WordView[]; text: string }) => `${fieldKey(item.words)}|${item.text}`;
  /** What says `item` wasn't saved (the caller says it: a toast or the "Updating…" line). */
  const unsaved = (item: QueuedSpelling) => {
    saidTexts.current.add(textKey(item));
    const text = item.text.trim().slice(0, 60);
    return t({
      id: 'captions.words.spellingUnsaved',
      message: `Your spelling “${text}” wasn’t saved: the words changed while you typed.`,
    });
  };
  /** Look presses that couldn't go, said in one toast once the close (or the list's) has run its rounds. */
  const lostLooks = useRef(0);
  const sayLost = (n: number) => {
    if (n === 0) return;
    if (lostLooks.current === 0)
      queueMicrotask(() => {
        const lost = lostLooks.current;
        lostLooks.current = 0;
        toast.info(
          t({
            id: 'captions.words.lookUnsaved',
            message: plural(lost, {
              one: 'A change to the words’ look wasn’t saved: the words changed before it could go in.',
              other: '# changes to the words’ look weren’t saved: the words changed before they could go in.',
            }),
          }),
        );
      });
    lostLooks.current += n;
  };
  /** Counts a commit this view sent until it lands. */
  const track = (fix: boolean, landed: Promise<boolean>) => {
    if (fix) fixing.current++;
    setSent((s) => ({ all: s.all + 1, fixes: s.fixes + (fix ? 1 : 0) }));
    void landed.finally(() => {
      if (fix) fixing.current--;
      setSent((s) => ({ all: s.all - 1, fixes: s.fixes - (fix ? 1 : 0) }));
    });
  };
  /** Sends a look press over `ws`: resolves with whether it went in; null when it changes nothing. */
  const sendLook = (change: LookChange, ws: WordView[]): Promise<boolean> | null => {
    if (lookOps(lookedAhead(ws), change).length === 0) return null;
    const n = ws.length;
    const by = ++sends.current;
    const build = () => {
      const looked = lookedAhead(ws);
      for (const w of looked) ahead.current.set(fileWordKey(w), { look: withChange(w.look, change), by });
      return lookOps(looked, change);
    };
    // Through the bar's burst: in its turn, kept at close (the list closed or not).
    const landed = burst.commit(lookLabel(change), build).then((ok) => {
      // Landed (or not): the project's looks say what it did now (before the commit after it is built).
      for (const [key, a] of ahead.current) if (a.by === by) ahead.current.delete(key);
      if (ok)
        announce(
          t({
            id: 'captions.words.changed',
            message: plural(n, {
              one: 'Changed the look of # word.',
              other: 'Changed the look of # words.',
            }),
          }),
        );
      return ok;
    });
    track(false, landed);
    return landed;
  };
  /**
   * Sends a spelling over `ws`, through the bar's burst: in its turn, kept at close, so the close waits for it (closing
   * closing or not, never ahead of a commit it waits for). A spelling the close waits
   * for (`hold`) lets go of the close's hold once its commit holds it.
   */
  const sendFix = (item: QueuedSpelling, ws: WordView[]) => {
    const ops = fixOps(ws, item.text, captionPunctuation());
    if (ops.length === 0) return item.hold?.release();
    sentFixes.current++;
    const landed = burst.commit(fixLabel(item), () => ops);
    item.hold?.release();
    track(
      true,
      landed.then((ok) => {
        if (ok) announce(t({ id: 'captions.words.fixed', message: 'Fixed the spelling.' }));
        return ok;
      }),
    );
  };

  /**
   * "Fix all": `ops` (every place a word shows, fixed as one undo step) through the bar's burst, counted as a fix of
   * this view, so presses after it wait for the words it makes. Only while nothing is kept or on its way (`busy`
   * false): the places were found on the words as they are now. False when it can't go now.
   */
  const fixAll = useStableCallback(
    (label: { id: 'captions.fixAll.undo'; params: Record<string, string | number> }, ops: Op[]): boolean => {
      if (busyNow() || gone.current || ops.length === 0) return false;
      setNote(null);
      sentFixes.current++;
      const landed = burst.commit(label, () => ops);
      const n = ops.length;
      track(
        true,
        landed.then((ok) => {
          if (ok)
            announce(
              t({
                id: 'captions.words.fixedAll',
                message: plural(n, { one: 'Fixed # word.', other: 'Fixed # words.' }),
              }),
            );
          return ok;
        }),
      );
      return true;
    },
  );

  /**
   * A look button pressed for the words selected. Going at once, it resolves with whether it went in; null when it is
   * kept until the words settle, or changes nothing (the look bar then takes its dry run off the video).
   */
  const press = useStableCallback((change: LookChange): Promise<boolean> | null => {
    const { words: ws, picked: pick } = live.current;
    if (ws.length === 0) return null;
    setNote(null);
    if (!busyNow()) return sendLook(change, ws);
    keep([
      ...queue.current,
      { kind: 'look', change, picked: pick, acts: acts(), lit: ws, fixes: drawnFixes.current },
    ]);
    // The list has closed (a colour's key settle flushing as it goes, after `settle`): nothing will drain it.
    if (gone.current) settle(false);
    return null;
  });
  /**
   * Spelling's save of `text` over the field's words (Enter, leaving it, or `closing`). While busy it waits (closing
   * too: the close waits for it); the same field saved again replaces what it had waiting. False, and said, when
   * it can't be kept: the list has closed, or the field still shows the words of the pick before while the one now
   * waits for its own. `repeat` (Enter on the text saved before) goes only when that one was said not to be saved.
   */
  const spell = useStableCallback(
    (ws: WordView[], text: string, from: string, closing?: boolean, repeat?: boolean): boolean => {
      const item: QueuedSpelling = { kind: 'spelling', words: ws, text, from };
      // Enter again on a text already sent or kept: nothing to do; on one said not to be saved: it goes again.
      if (repeat && !saidTexts.current.delete(textKey(item))) return true;
      setNote(null);
      if (!busyNow()) {
        sendFix(item, ws);
        return true;
      }
      if (gone.current || live.current.heldPick !== live.current.picked) {
        if (closing || gone.current) toast.info(unsaved(item));
        else tell(unsaved(item));
        return false;
      }
      if (closing) item.hold = keepAtClose(fixLabel(item));
      const at = queue.current.findIndex((q) => q.kind === 'spelling' && fieldKey(q.words) === fieldKey(ws));
      if (at >= 0) (queue.current[at] as QueuedSpelling).hold?.release();
      keep(at < 0 ? [...queue.current, item] : queue.current.map((q, k) => (k === at ? item : q)));
      return true;
    },
  );
  /** The person typed in Spelling: a press kept from before it no longer goes. */
  const typed = useStableCallback(() => {
    typing.current++;
    setNote(null);
  });

  // A new pick (or none) clears what was said; declared first, so what the queue says next shows.
  useEffect(() => setNote(null), [picked]);
  // Once the words have settled at the revision now and what this view sent has landed: what was kept goes.
  const flush = useStableCallback(() => {
    if (queue.current.length === 0 || pending || !fresh || sent.all > 0) return;
    const { steps, rest } = drain(queue.current, { picked, acts: acts(), words, find });
    keep(rest);
    for (const s of steps) {
      if (s.kind === 'look') sendLook(s.change, s.words);
      else if (s.kind === 'spelling') sendFix(s.item, s.words);
      else if (s.kind === 'dropped')
        tell(t({ id: 'captions.words.pressAgain', message: 'The words changed. Press again.' }));
      else if (s.item.hold && !s.item.hold.dropped) {
        // Said where it will still be seen as the project closes.
        s.item.hold.release();
        toast.info(unsaved(s.item));
      } else {
        // On the line (the close that gave up on it said it already, then failed).
        s.item.hold?.release();
        tell(unsaved(s.item));
      }
    }
  });
  useEffect(() => flush(), [flush, pending, fresh, sent.all, queued, picked, words]);
  /**
   * What is still kept as the project closes (`closing`), or the list does, by
   * `settleKept`'s rule: a look press goes now, each built on the presses before it, or is said in one toast; a spelling
   * waits for its pages while the project closes, the close waiting for it (`keepAtClose`), else is said; never lost
   * silently. True when something went (in its turn, kept at close), so closing goes round again (`flushBursts`).
   */
  const settle = useStableCallback((closing: boolean): boolean => {
    const { picked: pick } = live.current;
    const now = { picked: pick, acts: acts(), fixes: sentFixes.current, closing: closing && !gone.current };
    const { send, lost, unsaved: said, stay } = settleKept(queue.current, now);
    keep(stay);
    for (const q of stay) q.hold ??= keepAtClose(fixLabel(q));
    for (const q of said) {
      q.hold?.release();
      // Unless the close said it already, giving up on it.
      if (!q.hold?.dropped) toast.info(unsaved(q));
    }
    sayLost(lost);
    let went = false;
    for (const q of send) went = sendLook(q.change, q.lit) !== null || went;
    return went;
  });
  useEffect(() => {
    gone.current = false;
    const stop = registerBurst((closing) => closing && settle(true));
    return () => {
      stop();
      gone.current = true;
      settle(false);
    };
  }, [settle]);
  return { busy, note: busy ? null : note, press, spell, typed, fixAll };
}
