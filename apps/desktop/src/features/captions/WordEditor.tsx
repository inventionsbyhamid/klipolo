import { useLingui } from '@lingui/react/macro';
import { useEffect, useLayoutEffect, useRef, useState, type MouseEvent, type RefObject } from 'react';
import type { Op } from '../../bindings/Op';
import type { WordView } from '../../bindings/WordView';
import { registerBurst } from '../../state/bursts';
import { fixOp } from './captionOps';
import { FIX_MAX_CHARS, joinWords, visibleWords } from './wordFix';

/** Spaces as one, ends trimmed. */
const tidy = (s: string) => s.trim().replace(/\s+/g, ' ');

/** What comes before a word's first letter or digit, its letters and digits, and what comes after its last. */
export function around(text: string): [string, string, string] {
  const m = /^([^\p{L}\p{N}]*)(.*?)([^\p{L}\p{N}]*)$/su.exec(text);
  return m ? [m[1], m[2], m[3]] : ['', text, ''];
}

/** Whether two words are the same file words (a clip's copy plays the same ones: one fix covers both). */
const sameFileWords = (a: WordView, b: WordView) =>
  a.assetId === b.assetId && a.sourceStartUs === b.sourceStartUs && a.sourceEndUs === b.sourceEndUs;

/**
 * "Fix all": one `fixWords` per place the word shows (`matches`, from `captions_find`), in the file's sound time. The
 * word being edited reads exactly as typed; every other place keeps the marks around it ("Teo," reads "Theo,") around
 * the letters typed. None when nothing is left of what was typed but marks (taking a word out is a hide, not a fix).
 */
export function fixAllOps(word: WordView, matches: WordView[], typed: string): Op[] {
  const core = around(tidy(typed))[1];
  if (!core) return [];
  const all = matches.some((m) => sameFileWords(m, word)) ? matches : [word, ...matches];
  return all.flatMap((m) => {
    const said = m.said ?? m.text;
    const [before, , after] = around(said);
    const text = sameFileWords(m, word) ? tidy(typed) : `${before}${core}${after}`;
    if (text === said) return [];
    const op = fixOp([m], text);
    return op ? [op] : [];
  });
}

/** Spelling's save (`usePressQueue.spell`): true when it was sent or kept. */
type Spell = (words: WordView[], text: string, from: string, closing?: boolean, repeat?: boolean) => boolean;

/**
 * A word being fixed in place: the chip becomes a field with the word as said (never the style's
 * capitals), and the fix bar opens in the flow under its line: "Change Teo to Theo. Teo appears 4 times.", with Fix
 * all, Just this one and Hide. Enter fixes all (or just this one, when it shows once); Escape lets go without a change.
 * Leaving the field for anything but the bar saves the change for just this word, as does the project closing with it
 * open (the close waits for it) or the field going with the focus in it; a text saved already sends nothing twice. A
 * fix while the words are being updated waits for them (`usePressQueue`); "Fix all" waits for them to settle.
 */
export function WordEditor({
  word,
  matches,
  busy,
  spell,
  typed,
  fixAll,
  hide,
  close,
  list,
}: {
  word: WordView;
  /** Every place the word shows, once the app has said (null until then, or when it can't). */
  matches: WordView[] | null;
  busy: boolean;
  spell: Spell;
  typed: () => void;
  /** Sends "Fix all" (false when it can't go now). */
  fixAll: (label: { id: 'captions.fixAll.undo'; params: Record<string, string | number> }, ops: Op[]) => boolean;
  hide: () => void;
  /** The editor closes. */
  close: () => void;
  /** The word list, which takes the keyboard back when the field closes. */
  list: RefObject<HTMLDivElement | null>;
}) {
  const { t } = useLingui();
  const [from] = useState(() => joinWords(visibleWords([word])));
  const [text, setText] = useState(from);
  const field = useRef<HTMLInputElement>(null);
  const bar = useRef<HTMLDivElement>(null);
  /** The text last saved: leaving the field after Enter sends nothing twice. */
  const saved = useRef(from);
  const save = (now = false, enter = false) => {
    const repeat = text === saved.current;
    if (repeat && !enter) return false;
    saved.current = text;
    return spell([word], text, from, now, repeat);
  };
  const latest = useRef(save);
  latest.current = save;
  useEffect(() => registerBurst((closing) => closing && latest.current(true)), []);
  // Focused, its text selected, as it opens: the person asked for it. Its bar is brought into view under the line
  // (the click that opened it is over, so nothing moves under the pointer mid-click).
  useEffect(() => {
    field.current?.focus({ preventScroll: true });
    field.current?.select();
    bar.current?.scrollIntoView?.({ block: 'nearest' });
  }, []);
  // React sends no blur while it takes nodes off the page: a field going with the focus in it saves for itself.
  useLayoutEffect(() => {
    const input = field.current;
    return () => {
      if (input !== null && document.activeElement === input) latest.current();
    };
  }, []);
  const changed = tidy(text) !== tidy(from);
  const count = matches?.length ?? 0;
  const many = count > 1;
  const name = around(from)[1] || from;
  const to = tidy(text);
  const done = () => {
    saved.current = text;
    close();
    list.current?.focus({ preventScroll: true });
  };
  const justThis = () => {
    if (save(false, true)) done();
  };
  const all = () => {
    if (busy || !matches) return;
    const ops = fixAllOps(word, matches, text);
    const label = { id: 'captions.fixAll.undo' as const, params: { from: name, to: around(to)[1], count } };
    if (ops.length > 0 && fixAll(label, ops)) done();
  };
  const canFixAll = many && !busy && fixAllOps(word, matches ?? [], text).length > 0;
  /** The bar's buttons keep the focus in the field, so pressing one never reads as leaving it. */
  const keep = (e: MouseEvent) => e.preventDefault();
  return (
    <>
      <span className="word-edit">
        <input
          ref={field}
          aria-label={t({ id: 'captions.fix.word', message: `Fix the word ${name}` })}
          value={text}
          // A fix is exactly what is typed: no autocorrect, no capitals added, no spelling marks.
          spellCheck={false}
          autoCorrect="off"
          autoCapitalize="off"
          maxLength={from.length + FIX_MAX_CHARS}
          size={Math.max(4, text.length + 1)}
          onChange={(e) => {
            setText(e.target.value);
            typed();
          }}
          onBlur={(e) => {
            if (e.relatedTarget instanceof Node && bar.current?.contains(e.relatedTarget)) return;
            save();
            close();
          }}
          onKeyDown={(e) => {
            // The editor's keys (play, delete, split) never see the typing.
            e.stopPropagation();
            if (e.key === 'Enter') {
              e.preventDefault();
              if (!changed) done();
              else if (canFixAll) all();
              else if (!many) justThis();
            } else if (e.key === 'Escape') {
              e.preventDefault();
              done();
            }
          }}
        />
      </span>
      <div ref={bar} className="fix-bar" role="group" aria-label={t({ id: 'captions.fix.bar', message: 'Fix word' })}>
        <p>
          {changed ? (
            <>
              {t({ id: 'captions.fix.change', message: 'Change' })} <b>{name}</b>{' '}
              {t({ id: 'captions.fix.to', message: 'to' })} <b>{to || '…'}</b>.
            </>
          ) : (
            <>
              {t({ id: 'captions.fix.type', message: 'Type the right spelling for' })} <b>{name}</b>.
            </>
          )}
          {many && ` ${t({ id: 'captions.fix.count', message: `${name} appears ${count} times.` })}`}
        </p>
        <div className="actions">
          {changed && many && (
            <button
              type="button"
              className="btn btn-primary btn-sm"
              aria-disabled={!canFixAll || undefined}
              onMouseDown={keep}
              onClick={all}
            >
              {t({ id: 'captions.fix.all', message: `Fix all ${count}` })}
            </button>
          )}
          {changed && (
            <button
              type="button"
              className={many ? 'btn btn-secondary btn-sm' : 'btn btn-primary btn-sm'}
              onMouseDown={keep}
              onClick={justThis}
            >
              {many
                ? t({ id: 'captions.fix.one', message: 'Just this one' })
                : t({ id: 'captions.fix.save', message: 'Fix' })}
            </button>
          )}
          <button
            type="button"
            className="btn btn-ghost btn-sm"
            onMouseDown={keep}
            onClick={() => {
              saved.current = text;
              hide();
              close();
              list.current?.focus({ preventScroll: true });
            }}
          >
            {t({ id: 'captions.fix.hide', message: 'Hide' })}
          </button>
          <span className="fine">{t({ id: 'captions.fix.esc', message: 'Esc to cancel' })}</span>
        </div>
      </div>
    </>
  );
}
